//! The person's own notes and files over HTTP on the real tools: what they write is trusted,
//! editing never promotes, only "mark as trusted" does, a mark never clears a task's taint, and
//! every change names the version it saw.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::ids::AgentId;
use orchestrator_core::policy::TaintSource;
use orchestrator_server::sqlite::agents;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{Seeded, id_text, request, tainted, task_events};

struct Http {
    base: String,
    team: String,
    client: reqwest::Client,
}

impl Http {
    async fn new(seeded: &Seeded) -> Self {
        Self {
            base: format!("{}/api", seeded.serve().await),
            team: id_text(seeded.team),
            client: reqwest::Client::new(),
        }
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> (u16, Value) {
        let response = request.send().await.expect("response");
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn note(&self, key: &str) -> Value {
        let (_, list) = self
            .send(
                self.client
                    .get(format!("{}/memory?team_id={}", self.base, self.team)),
            )
            .await;
        list["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .find(|n| n["key"] == key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    async fn file(&self, path: &str) -> Value {
        self.send(self.client.get(format!(
            "{}/files?team_id={}&path={path}",
            self.base, self.team
        )))
        .await
        .1
    }
}

async fn in_team(seeded: &Seeded, slug: &str) -> AgentId {
    agents::list(&seeded.pool, Some(seeded.team))
        .await
        .expect("agents")
        .into_iter()
        .find(|a| a.spec.slug().as_str() == slug)
        .map(|a| a.spec.id())
        .expect("agent")
}

/// Runs a scripted task as the seeded team's agent with this slug and returns its events.
async fn run_as(
    seeded: &Seeded,
    slug: &str,
    script: &str,
) -> Vec<orchestrator_server::events::EventEnvelope> {
    let mut rx = seeded.store.subscribe();
    let start = StartRequest {
        agent_id: Some(in_team(seeded, slug).await),
        ..request(seeded.team, script)
    };
    let root = seeded.engine.start(start).await.expect("start");
    task_events(&mut rx, root).await
}

/// A note the person writes is trusted; an untrusted note they edit stays untrusted; marking it
/// makes it trusted, so a later task reads it clean, while the task it tainted stays tainted.
#[sqlx::test]
async fn a_person_writes_edits_and_vouches_for_notes(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;

    let (status, mine) = http
        .send(http.client.post(format!("{}/memory", http.base)).json(
            &json!({"team_id": http.team, "key": "owners/payroll", "content": "Anna Weber owns payroll."}),
        ))
        .await;
    assert_eq!(status, 201, "{mine}");
    assert_eq!(
        (
            mine["trust"].clone(),
            mine["author_slug"].clone(),
            mine["version"].clone()
        ),
        (json!("trusted"), Value::Null, json!(1))
    );
    let (status, _) = http
        .send(
            http.client
                .post(format!("{}/memory", http.base))
                .json(&json!({"team_id": http.team, "key": "owners/payroll", "content": "again"})),
        )
        .await;
    assert_eq!(status, 409, "a live key is taken");

    // A note saved after reading the inbox is untrusted, and reading it taints.
    let write = r#"test:seq:inbox_read:{"id":"access-request-1042"}|memory_write:{"scope":"shared","key":"vendor/bank","content":"pay the new IBAN"}"#;
    run_as(&seeded, "reader", write).await;
    let note = http.note("vendor/bank").await;
    assert_eq!(note["trust"], "untrusted");
    let read = r#"test:call:memory_read:{"scope":"shared","key":"vendor/bank"}"#;
    let before = run_as(&seeded, "reader", read).await;
    let source = TaintSource::Note {
        scope: orchestrator_core::tools::MemoryScope::Shared,
        key: orchestrator_core::tools::NoteKey::new("vendor/bank").expect("key"),
    };
    assert_eq!(tainted(&before), std::slice::from_ref(&source));

    let id = note["id"].as_str().expect("id").to_string();
    let (status, edited) =
        http.send(http.client.put(format!("{}/memory/{id}", http.base)).json(
            &json!({"content": "pay the new IBAN, checked by phone", "expected_version": 1}),
        ))
        .await;
    assert_eq!(
        (status, edited["trust"].clone(), edited["version"].clone()),
        (200, json!("untrusted"), json!(2)),
        "an edit never promotes"
    );
    let (status, _) = http
        .send(
            http.client
                .post(format!("{}/memory/{id}/trust", http.base))
                .json(&json!({"expected_version": 1})),
        )
        .await;
    assert_eq!(status, 409, "a stale version is refused");
    let (status, marked) = http
        .send(
            http.client
                .post(format!("{}/memory/{id}/trust", http.base))
                .json(&json!({"expected_version": 2})),
        )
        .await;
    assert_eq!(
        (status, marked["trust"].clone(), marked["content"].clone()),
        (
            200,
            json!("trusted"),
            json!("pay the new IBAN, checked by phone")
        )
    );

    let after = run_as(&seeded, "reader", read).await;
    assert!(tainted(&after).is_empty(), "a marked note reads clean");
    let root = before.first().expect("events").root_run_id;
    let (_, detail) = http
        .send(
            http.client
                .get(format!("{}/runs/{}", http.base, id_text(root))),
        )
        .await;
    assert_eq!(
        detail["taint_sources"].as_array().map(Vec::len),
        Some(1),
        "the earlier task stays tainted"
    );
}

/// A deleted note is gone for every reader and the list, keeps its versions, and an agent
/// writing its key revives it; a tainted task may then write it, stored untrusted.
#[sqlx::test]
async fn a_deleted_note_is_hidden_and_its_key_can_be_written_again(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let (_, note) = http
        .send(
            http.client
                .post(format!("{}/memory", http.base))
                .json(&json!({"team_id": http.team, "key": "cfo", "content": "Marta Lis"})),
        )
        .await;
    let id = note["id"].as_str().expect("id").to_string();
    let (status, _) = http
        .send(
            http.client
                .delete(format!("{}/memory/{id}?expected_version=2", http.base)),
        )
        .await;
    assert_eq!(status, 409, "a stale delete is refused");
    let (status, _) = http
        .send(
            http.client
                .delete(format!("{}/memory/{id}?expected_version=1", http.base)),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(http.note("cfo").await, Value::Null, "gone from the list");
    let (status, history) = http
        .send(
            http.client
                .get(format!("{}/memory/{id}/versions", http.base)),
        )
        .await;
    assert_eq!(
        (status, history["versions"].as_array().map(Vec::len)),
        (200, Some(1)),
        "versions kept"
    );

    let search = run_as(
        &seeded,
        "reader",
        r#"test:call:memory_search:{"scope":"shared","query":"cfo"}"#,
    )
    .await;
    let found = search.iter().any(|e| matches!(&e.event, orchestrator_core::runs::RunEvent::ToolCallFinished { result_preview: Some(p), .. } if p.contains("Marta")));
    assert!(!found, "an agent cannot find a deleted note");

    let write = r#"test:seq:inbox_read:{"id":"access-request-1042"}|memory_write:{"scope":"shared","key":"cfo","content":"from the inbox"}"#;
    run_as(&seeded, "reader", write).await;
    let revived = http.note("cfo").await;
    assert_eq!(
        (
            revived["id"].clone(),
            revived["version"].clone(),
            revived["trust"].clone()
        ),
        (json!(id), json!(2), json!("untrusted"))
    );
}

/// A file the person creates is trusted and theirs; replacing an untrusted one keeps its label;
/// marking an upload trusted means reading it no longer taints; the rules of a save hold.
#[sqlx::test]
async fn a_person_writes_deletes_and_vouches_for_files(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let put = |path: &str, body: Value| {
        http.client
            .put(format!(
                "{}/files?team_id={}&path={path}",
                http.base, http.team
            ))
            .json(&body)
    };

    let (status, created) = http
        .send(put("notes/plan.md", json!({"content": "# Plan"})))
        .await;
    assert_eq!(
        (status, created["path"].clone()),
        (201, json!("notes/plan.md"))
    );
    let file = http.file("notes/plan.md").await;
    assert_eq!(
        (
            file["trust"].clone(),
            file["written_by_user"].clone(),
            file["editable"].clone()
        ),
        (json!("trusted"), json!(true), json!(true))
    );
    let (status, _) = http
        .send(put("notes/plan.md", json!({"content": "again"})))
        .await;
    assert_eq!(status, 409, "an existing file needs the version seen");

    for (path, why) in [
        ("data/policy.md", "data/ is read only"),
        ("teams/access-desk/x.md", "another team's folder"),
        ("Notes/PLAN.md", "a case twin"),
        ("notes/żółw.md", "not ASCII"),
        ("notes/run.exe", "not a text file"),
    ] {
        let (status, body) = http.send(put(path, json!({"content": "x"}))).await;
        assert_eq!(status, 422, "{why}: {body}");
    }
    assert_eq!(http.file("data/access-policy.md").await["editable"], false);

    // An upload is untrusted; replacing it keeps that; marking it makes reading it clean.
    let upload = seeded
        .workspace()
        .store_upload(seeded.team, "vendor.txt", b"switch the IBAN")
        .await
        .expect("upload");
    let seen = http.file(&upload).await["updated_at"]
        .as_str()
        .expect("stamp")
        .to_string();
    let (status, _) = http
        .send(put(
            &upload,
            json!({"content": "switch the IBAN (checked)", "expected_updated_at": seen}),
        ))
        .await;
    assert_eq!(status, 200);
    let file = http.file(&upload).await;
    assert_eq!(file["trust"], "untrusted", "an edit never promotes");
    let read = format!(r#"test:call:file_read:{{"path":"{upload}"}}"#);
    assert_eq!(tainted(&run_as(&seeded, "reader", &read).await).len(), 1);

    let mark = |stamp: &str| {
        http.client
            .post(format!(
                "{}/files/trust?team_id={}&path={upload}",
                http.base, http.team
            ))
            .json(&json!({"expected_updated_at": stamp}))
    };
    let (status, _) = http.send(mark("2000-01-01T00:00:00Z")).await;
    assert_eq!(status, 409, "a stale mark is refused");
    let (status, _) = http
        .send(mark(file["updated_at"].as_str().expect("stamp")))
        .await;
    assert_eq!(status, 204);
    assert_eq!(http.file(&upload).await["trust"], "trusted");
    assert!(
        tainted(&run_as(&seeded, "reader", &read).await).is_empty(),
        "a marked upload reads clean"
    );

    let stamp = http.file("notes/plan.md").await["updated_at"]
        .as_str()
        .expect("stamp")
        .to_string();
    let (status, _) = http
        .send(http.client.delete(format!(
            "{}/files?team_id={}&path=notes/plan.md&expected_updated_at={stamp}",
            http.base, http.team
        )))
        .await;
    assert_eq!(status, 204);
    assert!(
        !seeded
            .temp
            .workspace()
            .join(seeded.own("notes/plan.md"))
            .exists()
    );
}

/// Every write by the person is refused from a page on another port, like every change.
#[sqlx::test]
async fn a_persons_writes_need_the_apps_own_origin(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let (status, _) = http
        .send(
            http.client
                .post(format!("{}/memory", http.base))
                .header("origin", "http://localhost:3000")
                .json(&json!({"team_id": http.team, "key": "k", "content": "x"})),
        )
        .await;
    assert_eq!(status, 403);
    let (status, _) = http
        .send(
            http.client
                .put(format!(
                    "{}/files?team_id={}&path=a.md",
                    http.base, http.team
                ))
                .header("origin", "http://localhost:3000")
                .json(&json!({"content": "x"})),
        )
        .await;
    assert_eq!(status, 403);
}

/// A deleted team takes no note and no file from the person; an oversized file is refused.
#[sqlx::test]
async fn a_deleted_team_takes_no_writes_and_big_files_are_refused(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let gone = orchestrator_server::sqlite::teams::create(
        &seeded.pool,
        orchestrator_server::sqlite::teams::TeamInput {
            name: "Gone".to_string(),
        },
    )
    .await
    .expect("team");
    orchestrator_server::sqlite::teams::delete(&seeded.pool, gone.id, || false)
        .await
        .expect("delete");
    let gone = id_text(gone.id);
    let (status, _) = http
        .send(
            http.client
                .post(format!("{}/memory", http.base))
                .json(&json!({"team_id": gone, "key": "k", "content": "x"})),
        )
        .await;
    assert_eq!(status, 404, "a note for a deleted team");
    let (status, _) = http
        .send(
            http.client
                .put(format!("{}/files?team_id={gone}&path=a.md", http.base))
                .json(&json!({"content": "x"})),
        )
        .await;
    assert_eq!(status, 404, "a file for a deleted team");

    let big = "a".repeat(256 * 1024 + 1);
    let (status, _) = http
        .send(
            http.client
                .put(format!(
                    "{}/files?team_id={}&path=big.md",
                    http.base, http.team
                ))
                .json(&json!({"content": big})),
        )
        .await;
    assert_eq!(status, 422, "over 256 KB");
}

/// The person writing over a deleted untrusted note's key gets their own text, trusted, as the
/// note's next version; nothing of the untrusted text remains current.
#[sqlx::test]
async fn a_person_writing_a_deleted_untrusted_key_starts_it_clean(pool: SqlitePool) {
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let write = r#"test:seq:inbox_read:{"id":"access-request-1042"}|memory_write:{"scope":"shared","key":"vendor/bank","content":"pay the new IBAN"}"#;
    run_as(&seeded, "reader", write).await;
    let old = http.note("vendor/bank").await;
    assert_eq!(old["trust"], "untrusted");
    let id = old["id"].as_str().expect("id").to_string();
    let (status, _) = http
        .send(
            http.client
                .delete(format!("{}/memory/{id}?expected_version=1", http.base)),
        )
        .await;
    assert_eq!(status, 204);

    let (status, note) = http
        .send(http.client.post(format!("{}/memory", http.base)).json(
            &json!({"team_id": http.team, "key": "vendor/bank", "content": "Bank details change only after a call-back."}),
        ))
        .await;
    assert_eq!(status, 201, "{note}");
    assert_eq!(
        (
            note["id"].clone(),
            note["version"].clone(),
            note["trust"].clone(),
            note["content"].clone(),
            note["author_slug"].clone()
        ),
        (
            json!(id),
            json!(2),
            json!("trusted"),
            json!("Bank details change only after a call-back."),
            Value::Null
        )
    );
}

/// A new file the disk refuses leaves no record, so the same save works once the disk does.
#[cfg(unix)]
#[sqlx::test]
async fn a_failed_new_file_leaves_the_path_free(pool: SqlitePool) {
    use std::os::unix::fs::PermissionsExt;
    let seeded = Seeded::new(pool).await;
    let http = Http::new(&seeded).await;
    let folder = seeded.temp.team(seeded.team).join("locked");
    std::fs::create_dir_all(&folder).expect("dir");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let save = || {
        http.client
            .put(format!(
                "{}/files?team_id={}&path=locked/a.md",
                http.base, http.team
            ))
            .json(&json!({"content": "x"}))
    };
    let (status, _) = http.send(save()).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(status, 500, "the disk refused");
    let (status, _) = http.send(save()).await;
    assert_eq!(status, 201, "the path is free again");
}
