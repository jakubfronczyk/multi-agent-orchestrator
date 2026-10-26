//! §13 test 12 and the file tools against a real temporary workspace.

use orchestrator_core::policy::{TaintSource, Trust};
use orchestrator_core::tools::{Tool, ToolCtx, ToolError};
use orchestrator_server::tools::csv::CsvQuery;
use orchestrator_server::tools::files::{FileList, FileRead, FileWrite};
use orchestrator_server::tools::workspace::Workspace;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{TempDir, app, id_text, tool_ctx};

async fn run(tool: &dyn Tool, ctx: &ToolCtx, args: Value) -> Result<Value, ToolError> {
    tool.check(&args)?;
    tool.run(ctx, args).await.map(|output| output.content)
}

#[cfg(unix)]
#[sqlx::test]
async fn file_tools_reject_traversal_and_symlinks(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "specialist").await;
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
    let own = temp.team(ctx.team_id);
    std::os::unix::fs::symlink(temp.0.join("secret.md"), own.join("link.md")).expect("file link");
    std::os::unix::fs::symlink(&temp.0, own.join("linkdir")).expect("dir link");

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
async fn writes_replace_atomically_and_untrusted_files_are_taint_sources(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let clean = tool_ctx(&app, "specialist").await;
    let tainted = ToolCtx {
        trust: Trust::Untrusted,
        ..tool_ctx(&app, "specialist").await
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
        .preview(
            clean.team_id,
            &json!({"path": "out/decision.md", "content": "deny"}),
        )
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
    let leftovers: Vec<_> = std::fs::read_dir(temp.team(clean.team_id).join("out"))
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
    let read_back = read
        .run(&clean, json!({"path": "out/note.md"}))
        .await
        .expect("read");
    assert_eq!(read_back.content["trust"], "untrusted");
    assert_eq!(
        read_back.taint,
        [TaintSource::File {
            path: "out/note.md".to_string()
        }]
    );
    let trusted = read
        .run(&clean, json!({"path": "out/decision.md"}))
        .await
        .expect("read");
    assert!(trusted.taint.is_empty(), "a trusted file taints nothing");
}

#[sqlx::test]
async fn listings_hide_temp_files_and_unrecorded_files_read_untrusted(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let clean = tool_ctx(&app, "specialist").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (read, list) = (FileRead::new(workspace.clone()), FileList::new(workspace));

    let own = temp.team(clean.team_id);
    std::fs::write(own.join("unrecorded.md"), "no row").expect("plain file");
    std::fs::write(own.join(".left.md.0194.tmp"), "aborted").expect("leftover");
    let listed = run(&list, &clean, json!({})).await.expect("list");
    let names: Vec<&str> = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    let unrecorded = run(&read, &clean, json!({"path": "unrecorded.md"}))
        .await
        .expect("read");
    assert_eq!(
        unrecorded["trust"], "untrusted",
        "a file with no trust row fails closed"
    );
}

#[sqlx::test]
async fn writes_need_a_text_extension_and_at_most_256_kb(pool: SqlitePool) {
    let temp = TempDir::new();
    let write = FileWrite::new(Workspace::new(temp.workspace(), pool));
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
    let ctx = tool_ctx(&app, "specialist").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (write, csv) = (FileWrite::new(workspace.clone()), CsvQuery::new(workspace));
    let systems =
        "system,owner,seats\nPayroll,Anna Weber,12\nCRM,Rolf Bauer,40\nWiki,Anna Weber,300\n";
    run(
        &write,
        &ctx,
        json!({"path": "lists/systems.csv", "content": systems}),
    )
    .await
    .expect("write");

    let anna = run(&csv, &ctx, json!({"path": "lists/systems.csv", "filter": [{"column": "owner", "op": "eq", "value": "Anna Weber"}], "select": ["system"]})).await.expect("query");
    assert_eq!(anna["rows"], json!([["Payroll"], ["Wiki"]]));
    let big = run(&csv, &ctx, json!({"path": "lists/systems.csv", "filter": [{"column": "seats", "op": "gt", "value": "20"}], "count": true})).await.expect("query");
    assert_eq!(big["count"], 2, "numbers compare as numbers");
    let grouped = run(
        &csv,
        &ctx,
        json!({"path": "lists/systems.csv", "group_by": "owner"}),
    )
    .await
    .expect("query");
    assert_eq!(grouped["counts"]["Anna Weber"], 2);
    let defaults = run(
        &csv,
        &ctx,
        json!({"path": "lists/systems.csv", "group_by": "", "select": [""], "count": false, "filter": []}),
    )
    .await
    .expect("empty fields mean absent");
    assert_eq!(defaults["total"], 3);
    assert_eq!(defaults["columns"], json!(["system", "owner", "seats"]));

    let unknown = run(
        &csv,
        &ctx,
        json!({"path": "lists/systems.csv", "select": ["salary"]}),
    )
    .await
    .expect_err("unknown column");
    assert_eq!(unknown.to_string(), "unknown column: salary");
    assert!(
        csv.check(&json!({"path": "lists/systems.csv", "where": "1=1"}))
            .is_err(),
        "no expressions"
    );
}

#[sqlx::test]
async fn a_write_whose_final_trust_update_fails_reads_back_untrusted(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "specialist").await;
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

    let read_back = read
        .run(&ctx, json!({"path": "out/a.md"}))
        .await
        .expect("read");
    assert_eq!(read_back.content["trust"], "untrusted");
    assert_eq!(
        read_back.taint,
        [TaintSource::File {
            path: "out/a.md".to_string()
        }]
    );
}

/// A trust value that does not decode fails closed: the file reads, lists and queries as untrusted.
#[sqlx::test]
async fn an_unknown_stored_trust_reads_as_untrusted(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "specialist").await;
    let temp = TempDir::new();
    std::fs::write(temp.team(ctx.team_id).join("odd.csv"), "a\n1\n").expect("file");
    let physical = format!("teams/{}/odd.csv", id_text(ctx.team_id));
    sqlx::query(
        "INSERT INTO workspace_files (path, trust, updated_at)
         VALUES (?, 'TRUSTED', '2026-01-01T00:00:00.000000Z')",
    )
    .bind(&physical)
    .execute(&pool)
    .await
    .expect("row");
    let workspace = Workspace::new(temp.workspace(), pool);
    let source = [TaintSource::File {
        path: "odd.csv".to_string(),
    }];

    let read = FileRead::new(workspace.clone())
        .run(&ctx, json!({"path": "odd.csv"}))
        .await
        .expect("read");
    assert_eq!(read.content["trust"], "untrusted");
    assert_eq!(read.taint, source);
    let query = CsvQuery::new(workspace.clone())
        .run(&ctx, json!({"path": "odd.csv"}))
        .await
        .expect("query");
    assert_eq!(query.taint, source);
    let list = FileList::new(workspace.clone())
        .run(&ctx, json!({}))
        .await
        .expect("list");
    assert_eq!(list.taint, source);
    let (_, trust, _) = workspace.read(&physical).await.expect("view");
    assert_eq!(trust, Trust::Untrusted);
}

/// A path that differs from a recorded file only in letter case names the same file on a
/// case-insensitive disk, so a tainted write there must not leave the trusted row in place.
#[sqlx::test]
async fn a_case_variant_path_cannot_hide_tainted_bytes_behind_a_trusted_row(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let clean = tool_ctx(&app, "specialist").await;
    let tainted = ToolCtx {
        trust: Trust::Untrusted,
        ..tool_ctx(&app, "specialist").await
    };
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (read, write) = (FileRead::new(workspace.clone()), FileWrite::new(workspace));

    run(
        &write,
        &clean,
        json!({"path": "reports/shared.md", "content": "first"}),
    )
    .await
    .expect("write");
    let twin = run(
        &write,
        &tainted,
        json!({"path": "Reports/SHARED.md", "content": "from the inbox"}),
    )
    .await;
    assert!(twin.is_err(), "a case twin of a recorded file is refused");
    let back = run(&read, &clean, json!({"path": "reports/shared.md"}))
        .await
        .expect("read");
    assert_eq!(
        (back["content"].clone(), back["trust"].clone()),
        (json!("first"), json!("trusted"))
    );

    let non_ascii = json!({"path": "reports/ﬁle.md", "content": "x"});
    assert!(write.check(&non_ascii).is_err(), "a saved path is ASCII");
}

/// `sum` adds a column exactly over the matching rows, per group when grouped, and names the row
/// of a cell that is not a number.
#[sqlx::test]
async fn csv_query_sums_a_column(pool: SqlitePool) {
    let app = app(pool.clone()).await;
    let ctx = tool_ctx(&app, "specialist").await;
    let temp = TempDir::new();
    let workspace = Workspace::new(temp.workspace(), pool);
    let (write, csv) = (FileWrite::new(workspace.clone()), CsvQuery::new(workspace));
    let invoices = "invoice,vendor,amount\nI-1,Acme,1200.00\nI-2,Bolt,16699.00\nI-3,Acme,0.50\n";
    run(
        &write,
        &ctx,
        json!({"path": "lists/invoices.csv", "content": invoices}),
    )
    .await
    .expect("write");

    let total = run(
        &csv,
        &ctx,
        json!({"path": "lists/invoices.csv", "sum": "amount"}),
    )
    .await
    .expect("sum");
    assert_eq!(
        (total["sum"].clone(), total["rows"].clone()),
        (json!("17899.50"), json!(3))
    );
    let acme = run(&csv, &ctx, json!({"path": "lists/invoices.csv", "sum": "amount", "filter": [{"column": "vendor", "op": "eq", "value": "Acme"}]}))
        .await
        .expect("sum");
    assert_eq!(acme["sum"], "1200.50");
    let grouped = run(
        &csv,
        &ctx,
        json!({"path": "lists/invoices.csv", "sum": "amount", "group_by": "vendor"}),
    )
    .await
    .expect("sum");
    assert_eq!(
        grouped["sums"],
        json!({"Acme": "1200.50", "Bolt": "16699.00"})
    );
    let words = run(
        &csv,
        &ctx,
        json!({"path": "lists/invoices.csv", "sum": "vendor"}),
    )
    .await
    .expect_err("not numbers");
    assert!(words.to_string().contains("row 1"), "{words}");
}
