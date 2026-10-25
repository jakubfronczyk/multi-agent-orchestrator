//! `POST /api/files` on the real workspace: an upload is stored untrusted under `uploads/`,
//! never outside it and never over another upload, and a task that reads it is tainted.

use std::sync::Arc;

use orchestrator_core::engine::{Engine, StartRequest};
use orchestrator_core::policy::TaintSource;
use orchestrator_core::runs::RunEvent;
use orchestrator_server::api::{ActiveProvider, AppState};
use orchestrator_server::llm::ScriptedProvider;
use orchestrator_server::sqlite::{SqliteStore, agents};
use orchestrator_server::tools::files::Workspace;
use orchestrator_server::{seed, tools};
use serde_json::Value;
use sqlx::SqlitePool;

use crate::{TempDir, serve_state, task_events, test_config};

const BOUNDARY: &str = "upload-boundary";

struct Served {
    pool: SqlitePool,
    store: Arc<SqliteStore>,
    engine: Arc<Engine>,
    temp: TempDir,
    base: String,
}

async fn served(pool: SqlitePool) -> Served {
    let temp = TempDir::new();
    assert!(seed::run(&pool, &temp.workspace()).await.expect("seed"));
    let store = Arc::new(SqliteStore::new(pool.clone()));
    let tools = tools::all(pool.clone(), Workspace::new(temp.workspace(), pool.clone()));
    let engine = Arc::new(Engine::new(
        store.clone(),
        Arc::new(ScriptedProvider::new()),
        tools,
        test_config(),
    ));
    let base = serve_state(AppState {
        pool: pool.clone(),
        store: store.clone(),
        engine: engine.clone(),
        provider: ActiveProvider::Scripted,
        workspace: Workspace::new(temp.workspace(), pool.clone()),
    })
    .await;
    Served {
        pool,
        store,
        engine,
        temp,
        base,
    }
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

async fn upload(base: &str, body: Vec<u8>) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}/api/files"))
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
    let served = served(pool).await;
    let (status, json) = upload(&served.base, body("file", "notes.md", b"# Notes\n")).await;
    assert_eq!(status, 201, "{json}");
    assert_eq!(json["path"], "uploads/notes.md");
    assert_eq!(json["bytes"], 8);
    assert_eq!(json["trust"], "untrusted");
    let on_disk =
        std::fs::read_to_string(served.temp.workspace().join("uploads/notes.md")).expect("stored");
    assert_eq!(on_disk, "# Notes\n");
    let row = sqlx::query!(
        "SELECT trust, written_by_run FROM workspace_files WHERE path = 'uploads/notes.md'"
    )
    .fetch_one(&served.pool)
    .await
    .expect("row");
    assert_eq!(
        (row.trust.as_str(), row.written_by_run),
        ("untrusted", None)
    );

    let view: Value = reqwest::get(format!("{}/api/files?path=uploads/notes.md", served.base))
        .await
        .expect("response")
        .json()
        .await
        .expect("json");
    assert_eq!(view["trust"], "untrusted");
    assert!(view["written_by_root_run_id"].is_null());

    let (status, _) = upload(&served.base, body("file", "notes.md", b"replaced")).await;
    assert_eq!(status, 409, "an upload never replaces one");
    let kept =
        std::fs::read_to_string(served.temp.workspace().join("uploads/notes.md")).expect("stored");
    assert_eq!(kept, "# Notes\n");
}

#[sqlx::test]
async fn bad_uploads_are_refused_before_anything_is_written(pool: SqlitePool) {
    let served = served(pool).await;
    for name in [
        "../x.md",
        "a/b.md",
        ".env",
        "",
        "x y.md",
        "a\\b.md",
        &format!("{}.md", "a".repeat(98)),
    ] {
        let (status, json) = upload(&served.base, body("file", name, b"x")).await;
        assert_eq!(status, 422, "{name:?}: {json}");
        assert_eq!(json["error"]["field"], "file", "{name:?}");
    }
    let (status, _) = upload(&served.base, body("other", "x.md", b"x")).await;
    assert_eq!(status, 422, "the field must be named file");

    let big = vec![b'a'; 1024 * 1024 + 1];
    let (status, _) = upload(&served.base, body("file", "big.txt", &big)).await;
    assert_eq!(status, 413);
    let (status, _) = upload(&served.base, body("file", "bin.txt", &[0xff, 0xfe, 0x00])).await;
    assert_eq!(status, 415, "not UTF-8");
    let (status, _) = upload(&served.base, body("file", "image.png", b"text")).await;
    assert_eq!(status, 415, "not a text extension");

    assert!(!served.temp.workspace().join("x.md").exists());
    let names: Vec<String> = std::fs::read_dir(served.temp.workspace().join("uploads"))
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(names.is_empty(), "nothing was stored: {names:?}");
    let rows =
        sqlx::query_scalar!("SELECT COUNT(*) FROM workspace_files WHERE path LIKE 'uploads/%'")
            .fetch_one(&served.pool)
            .await
            .expect("count");
    assert_eq!(rows, 0);
}

#[sqlx::test]
async fn a_symlinked_uploads_directory_is_refused(pool: SqlitePool) {
    let served = served(pool).await;
    let outside = TempDir::new();
    std::fs::create_dir_all(outside.workspace()).expect("dir");
    std::os::unix::fs::symlink(outside.workspace(), served.temp.workspace().join("uploads"))
        .expect("symlink");
    let (status, json) = upload(&served.base, body("file", "notes.md", b"x")).await;
    assert_eq!(status, 422, "{json}");
    assert!(!outside.workspace().join("notes.md").exists());
}

/// Every way a task can read an upload taints it with the file, as an inbox message would.
#[sqlx::test]
async fn reading_an_upload_taints_the_task(pool: SqlitePool) {
    let served = served(pool).await;
    let (status, _) = upload(
        &served.base,
        body("file", "people.csv", b"name,team\nAda,payroll\n"),
    )
    .await;
    assert_eq!(status, 201);
    let reader = agents::list(&served.pool)
        .await
        .expect("agents")
        .into_iter()
        .find(|record| record.spec.slug().as_str() == "reader")
        .expect("reader")
        .spec
        .id();
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
                task: "read the upload".to_string(),
                title: None,
                budget: None,
                scenario: Some(script.to_string()),
                draft_only: false,
            })
            .await
            .expect("start");
        let tainted: Vec<TaintSource> = task_events(&mut rx, root)
            .await
            .into_iter()
            .filter_map(|e| match e.event {
                RunEvent::TaskTainted { source } => Some(source),
                _ => None,
            })
            .collect();
        assert_eq!(tainted, std::slice::from_ref(&upload_source), "{script}");
    }
}
