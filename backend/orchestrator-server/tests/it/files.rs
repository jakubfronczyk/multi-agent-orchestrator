//! §13 test 12 and the file tools against a real temporary workspace.

use std::path::PathBuf;

use orchestrator_core::policy::Trust;
use orchestrator_core::tools::{Tool, ToolCtx, ToolError};
use orchestrator_server::tools::files::{CsvQuery, FileList, FileRead, FileWrite, Workspace};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{app, tool_ctx};

/// A fresh directory per test, removed when the guard drops.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("orchestrator-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("workspace")).expect("temp dir");
        Self(dir)
    }

    fn workspace(&self) -> PathBuf {
        self.0.join("workspace")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn run(tool: &dyn Tool, ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    tool.check(&args)?;
    tool.run(ctx, args).await.map(|output| output.content)
}

#[cfg(unix)]
#[sqlx::test]
async fn file_tools_reject_traversal_and_symlinks(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "worker").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (read, write, list) = (
        FileRead::new(workspace.clone()),
        FileWrite::new(workspace.clone()),
        FileList::new(workspace),
    );

    for path in ["../secret.md", "/etc/passwd", "a/../../secret.md"] {
        assert!(read.check(&json!({ "path": path })).is_err(), "{path}");
        assert!(
            write
                .check(&json!({ "path": path, "content": "x" }))
                .is_err(),
            "{path}"
        );
        assert!(list.check(&json!({ "path": path })).is_err(), "{path}");
    }

    std::fs::write(temp.0.join("secret.md"), "outside the workspace").expect("outside file");
    std::os::unix::fs::symlink(temp.0.join("secret.md"), temp.workspace().join("link.md"))
        .expect("file link");
    std::os::unix::fs::symlink(&temp.0, temp.workspace().join("linkdir")).expect("dir link");

    for path in ["link.md", "linkdir/secret.md"] {
        let error = run(&read, &ctx, json!({ "path": path }))
            .await
            .expect_err("rejected");
        assert!(error.to_string().contains("symlinks"), "{path}: {error}");
        assert!(
            !error.to_string().contains("outside the workspace"),
            "no file bytes in errors"
        );
    }
    let through = run(
        &write,
        &ctx,
        json!({ "path": "linkdir/planted.md", "content": "x" }),
    )
    .await;
    assert!(through.is_err());
    assert!(
        !temp.0.join("planted.md").exists(),
        "nothing written outside the workspace"
    );
}

#[sqlx::test]
async fn writes_replace_atomically_and_reads_wrap_untrusted_files(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let clean = tool_ctx(&app, "worker").await;
    let tainted = ToolCtx {
        trust: Trust::Untrusted,
        ..tool_ctx(&app, "worker").await
    };
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (read, write) = (FileRead::new(workspace.clone()), FileWrite::new(workspace));

    let first = run(
        &write,
        &clean,
        json!({"path": "out/decision.md", "content": "approve"}),
    )
    .await
    .expect("write");
    assert_eq!(first["replaced"], false);
    let preview = write
        .preview(&json!({"path": "out/decision.md", "content": "deny"}))
        .await;
    assert!(
        preview.contains("deny") && preview.contains("(replaces an existing file)"),
        "{preview}"
    );
    let second = run(
        &write,
        &clean,
        json!({"path": "out/decision.md", "content": "deny"}),
    )
    .await
    .expect("write");
    assert_eq!(second["replaced"], true);
    let leftovers: Vec<_> = std::fs::read_dir(temp.workspace().join("out"))
        .expect("dir")
        .collect();
    assert_eq!(leftovers.len(), 1, "no temp file is left behind");

    let trusted = run(&read, &clean, json!({"path": "out/decision.md"}))
        .await
        .expect("read");
    assert_eq!(trusted["content"], "deny");

    run(
        &write,
        &tainted,
        json!({"path": "out/note.md", "content": "from the inbox"}),
    )
    .await
    .expect("write");
    let wrapped = run(&read, &clean, json!({"path": "out/note.md"}))
        .await
        .expect("read");
    assert_eq!(wrapped["trust"], "untrusted");
    assert!(
        wrapped["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("<untrusted_data source=\"file:out/note.md\">"))
    );

    std::fs::write(temp.workspace().join("unrecorded.md"), "no row").expect("plain file");
    let unrecorded = run(&read, &clean, json!({"path": "unrecorded.md"}))
        .await
        .expect("read");
    assert_eq!(
        unrecorded["trust"], "untrusted",
        "a file with no trust row fails closed"
    );

    for args in [
        json!({"path": "run.exe", "content": "x"}),
        json!({"path": "big.md", "content": "x".repeat(256 * 1024 + 1)}),
    ] {
        assert!(write.check(&args).is_err(), "{args}");
    }
}

#[sqlx::test]
async fn csv_query_filters_selects_counts_and_groups(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "worker").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (write, csv) = (FileWrite::new(workspace.clone()), CsvQuery::new(workspace));
    let systems =
        "system,owner,seats\nPayroll,Anna Weber,12\nCRM,Rolf Bauer,40\nWiki,Anna Weber,300\n";
    run(
        &write,
        &ctx,
        json!({"path": "data/systems.csv", "content": systems}),
    )
    .await
    .expect("write");

    let anna = run(&csv, &ctx, json!({"path": "data/systems.csv", "filter": [{"column": "owner", "op": "eq", "value": "Anna Weber"}], "select": ["system"]})).await.expect("query");
    assert_eq!(anna["rows"], json!([["Payroll"], ["Wiki"]]));
    let big = run(&csv, &ctx, json!({"path": "data/systems.csv", "filter": [{"column": "seats", "op": "gt", "value": "20"}], "count": true})).await.expect("query");
    assert_eq!(big["count"], 2, "numbers compare as numbers");
    let grouped = run(
        &csv,
        &ctx,
        json!({"path": "data/systems.csv", "group_by": "owner"}),
    )
    .await
    .expect("query");
    assert_eq!(grouped["counts"]["Anna Weber"], 2);

    let unknown = run(
        &csv,
        &ctx,
        json!({"path": "data/systems.csv", "select": ["salary"]}),
    )
    .await
    .expect_err("unknown column");
    assert_eq!(unknown.to_string(), "unknown column: salary");
    assert!(
        csv.check(&json!({"path": "data/systems.csv", "where": "1=1"}))
            .is_err(),
        "no expressions"
    );
}

#[sqlx::test]
async fn a_write_whose_final_trust_update_fails_reads_back_untrusted(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "worker").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool.clone());
    let (read, write) = (FileRead::new(workspace.clone()), FileWrite::new(workspace));

    // Only the last step, setting the row to trusted, can fail now.
    sqlx::raw_sql(
        "CREATE TRIGGER fail_trusted BEFORE UPDATE ON workspace_files WHEN NEW.trust = 'trusted'
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )
    .execute(&pool)
    .await
    .expect("trigger");
    assert!(
        run(
            &write,
            &ctx,
            json!({"path": "out/a.md", "content": "new bytes"})
        )
        .await
        .is_err()
    );

    let read_back = run(&read, &ctx, json!({"path": "out/a.md"}))
        .await
        .expect("read");
    assert_eq!(read_back["trust"], "untrusted");
    assert!(
        read_back["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("<untrusted_data"))
    );
}
