//! `POST /api/files` on the real workspace: an upload is stored untrusted under the team's
//! `uploads/`, never outside it and never over another upload, and a task that reads it is tainted.

use orchestrator_core::engine::StartRequest;
use orchestrator_core::policy::TaintSource;
use orchestrator_server::tools::workspace::FILE_BYTES;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{Seeded, TempDir, agent_id, id_text, request, tainted, task_events};

const BOUNDARY: &str = "upload-boundary";

/// The seeded team and its files endpoint, `/api/files?team_id=<seeded team>`.
async fn served(pool: SqlitePool) -> (Seeded, String) {
    let seeded = Seeded::new(pool).await;
    let files = format!(
        "{}/api/files?team_id={}",
        seeded.serve().await,
        id_text(seeded.team)
    );
    (seeded, files)
}

/// One multipart field named `field` holding `bytes` as the file `name`.
fn body(field: &str, name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn upload(files: &str, body: Vec<u8>) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(files)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .send()
        .await
        .expect("response");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[sqlx::test]
async fn an_upload_is_stored_untrusted_under_uploads(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let (status, json) = upload(&base, body("file", "notes.md", b"# Notes\n")).await;
    assert_eq!(status, 201, "{json}");
    assert_eq!(json["path"], "uploads/notes.md");
    assert_eq!(json["bytes"], 8);
    assert_eq!(json["trust"], "untrusted");
    let on_disk =
        std::fs::read_to_string(served.temp.workspace().join(served.own("uploads/notes.md")))
            .expect("stored");
    assert_eq!(on_disk, "# Notes\n");
    let (trust, written_by_run): (String, Option<String>) =
        sqlx::query_as("SELECT trust, written_by_run FROM workspace_files WHERE path = ?")
            .bind(served.own("uploads/notes.md"))
            .fetch_one(&served.pool)
            .await
            .expect("row");
    assert_eq!((trust.as_str(), written_by_run), ("untrusted", None));

    let view: Value = reqwest::get(format!("{base}&path=uploads/notes.md"))
        .await
        .expect("response")
        .json()
        .await
        .expect("json");
    assert_eq!(view["trust"], "untrusted");
    assert!(view["written_by_root_run_id"].is_null());

    let (status, again) = upload(&base, body("file", "notes.md", b"replaced")).await;
    assert_eq!(
        (status, again["path"].clone()),
        (201, json!("uploads/notes-2.md")),
        "a taken name gets a number"
    );
    let kept =
        std::fs::read_to_string(served.temp.workspace().join(served.own("uploads/notes.md")))
            .expect("stored");
    assert_eq!(kept, "# Notes\n", "an upload never replaces one");
}

/// Any name is cleaned rather than refused; only binary or oversized files are refused, before
/// anything is written.
#[sqlx::test]
async fn any_name_is_cleaned_and_only_binary_or_big_files_are_refused(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    for (raw, stored) in [
        ("../x.md", "uploads/x.md"),
        ("Raport Q3 (final).CSV", "uploads/Raport-Q3-final.CSV"),
        ("data.tsv", "uploads/data.tsv"),
        (".env", "uploads/env"),
    ] {
        let (status, json) = upload(&base, body("file", raw, b"x")).await;
        assert_eq!(
            (status, json["path"].clone()),
            (201, json!(stored)),
            "{raw:?}"
        );
    }
    assert!(
        !served.temp.workspace().join("x.md").exists(),
        "nothing outside the folder"
    );

    let (status, _) = upload(&base, body("other", "y.md", b"x")).await;
    assert_eq!(status, 422, "the field must be named file");
    let big = vec![b'a'; FILE_BYTES + 1];
    let (status, _) = upload(&base, body("file", "big.txt", &big)).await;
    assert_eq!(status, 413, "one byte over the read cap");
    let (status, _) = upload(&base, body("file", "bin.txt", &[0xff, 0xfe, 0x00])).await;
    assert_eq!(status, 415, "not UTF-8");
    let (status, _) = upload(&base, body("file", "image.png", b"PNG\0\0")).await;
    assert_eq!(status, 415, "binary");
    for refused in ["y.md", "big.txt", "bin.txt", "image.png"] {
        let path = served
            .temp
            .workspace()
            .join(served.own(&format!("uploads/{refused}")));
        assert!(!path.exists(), "{refused} was stored");
    }
}

#[cfg(unix)]
#[sqlx::test]
async fn a_symlinked_uploads_directory_is_refused(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let outside = TempDir::new();
    std::fs::create_dir_all(outside.workspace()).expect("dir");
    std::os::unix::fs::symlink(
        outside.workspace(),
        served.temp.team(served.team).join("uploads"),
    )
    .expect("symlink");
    let (status, json) = upload(&base, body("file", "notes.md", b"x")).await;
    assert_eq!(status, 422, "{json}");
    assert!(!outside.workspace().join("notes.md").exists());
}

#[cfg(unix)]
#[sqlx::test]
async fn an_upload_the_disk_refuses_is_a_server_error(pool: SqlitePool) {
    use std::os::unix::fs::PermissionsExt;
    let (served, base) = served(pool).await;
    let uploads = served.temp.team(served.team).join("uploads");
    std::fs::create_dir_all(&uploads).expect("dir");
    std::fs::set_permissions(&uploads, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let (status, json) = upload(&base, body("file", "notes.md", b"x")).await;
    std::fs::set_permissions(&uploads, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    assert_eq!(status, 500, "{json}");
    assert_eq!(json["error"]["code"], "internal");
}

/// An upload at the cap is stored and read back whole through the file tools' reader.
#[sqlx::test]
async fn an_upload_at_the_cap_is_readable(pool: SqlitePool) {
    let (_served, base) = served(pool).await;
    let full = vec![b'a'; FILE_BYTES];
    let (status, json) = upload(&base, body("file", "full.txt", &full)).await;
    assert_eq!(status, 201, "{json}");
    let view: Value = reqwest::get(format!("{base}&path=uploads/full.txt"))
        .await
        .expect("response")
        .json()
        .await
        .expect("json");
    assert_eq!(view["content"].as_str().map(str::len), Some(FILE_BYTES));
}

/// Every way a task can read an upload taints it with the file, as an inbox message would.
#[sqlx::test]
async fn reading_an_upload_taints_the_task(pool: SqlitePool) {
    let (served, base) = served(pool).await;
    let (status, _) = upload(
        &base,
        body("file", "people.csv", b"name,team\nAda,payroll\n"),
    )
    .await;
    assert_eq!(status, 201);
    let reader = agent_id(&served.pool, "reader").await;
    let upload_source = TaintSource::File {
        path: "uploads/people.csv".to_string(),
    };
    for script in [
        r#"test:call:file_read:{"path":"uploads/people.csv"}"#,
        r#"test:call:csv_query:{"path":"uploads/people.csv"}"#,
        r#"test:call:file_list:{"path":"uploads"}"#,
    ] {
        let mut rx = served.store.subscribe();
        let root = served
            .engine
            .start(StartRequest {
                agent_id: Some(reader),
                ..request(served.team, script)
            })
            .await
            .expect("start");
        let events = task_events(&mut rx, root).await;
        assert_eq!(
            tainted(&events),
            std::slice::from_ref(&upload_source),
            "{script}"
        );
    }
}
