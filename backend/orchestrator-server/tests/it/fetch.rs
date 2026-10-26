//! §13 test 11: http_fetch never reaches a non-public address, whatever form the URL takes.

use orchestrator_core::tools::{Target, Tool};
use orchestrator_server::tools::fetch::HttpFetch;
use serde_json::json;
use sqlx::SqlitePool;

use crate::{app, tool_ctx};

#[sqlx::test]
async fn http_fetch_blocks_private_addresses_and_odd_forms(pool: SqlitePool) {
    let app = app(pool).await;
    let ctx = tool_ctx(&app, "reader").await;
    let fetch = HttpFetch::new();

    // Parsing normalizes odd forms, so the host the allowlist and the card see is the real one.
    for (url, host) in [
        ("http://2130706433/", "127.0.0.1"),
        ("http://0x7f.1/", "127.0.0.1"),
        ("http://[::ffff:127.0.0.1]/", "::ffff:7f00:1"),
    ] {
        let action = fetch.check(&json!({ "url": url })).expect("valid url");
        assert!(
            matches!(action.target, Target::Host { host: ref h, .. } if h == host),
            "{url} -> {:?}",
            action.target
        );
    }

    for url in [
        "http://127.0.0.1/",
        "http://2130706433/",
        "http://0x7f.1/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::1]:8080/",
        "http://localhost/",
        "http://10.0.0.1/",
        "http://169.254.169.254/latest/meta-data",
        "http://100.64.0.1/",
        "http://[fd00::1]/",
        "http://0.0.0.0/",
    ] {
        let error = fetch
            .run(&ctx, json!({ "url": url }))
            .await
            .expect_err("blocked");
        assert!(error.to_string().contains("not public"), "{url}: {error}");
    }

    for url in [
        "file:///etc/passwd",
        "ftp://example.com/x",
        "not a url",
        "http://",
    ] {
        assert!(fetch.check(&json!({ "url": url })).is_err(), "{url}");
    }
    assert!(
        fetch
            .check(&json!({ "url": "https://example.com", "method": "POST" }))
            .is_err(),
        "GET only"
    );
}

/// Live sites, so ignored by default: `cargo test -p orchestrator-server --test it -- --ignored live_`.
/// A Wikipedia page comes back as its title and readable text, and a redirect to another host
/// is reported, not followed.
#[tokio::test]
#[ignore = "needs the network"]
async fn live_pages_come_back_as_readable_text() {
    use orchestrator_core::ids::{AgentId, RunId, TeamId, ToolCallId};
    use orchestrator_core::policy::Trust;
    use orchestrator_core::tools::{Tool, ToolCtx};
    let run = RunId::new();
    let ctx = ToolCtx {
        run_id: run,
        root_run_id: run,
        call_id: ToolCallId::new(),
        agent_id: AgentId::new(),
        team_id: TeamId::new(),
        role: orchestrator_core::agents::Role::Specialist,
        trust: Trust::Trusted,
    };
    let fetch = HttpFetch::new();
    let wiki = fetch
        .run(
            &ctx,
            serde_json::json!({"url": "https://en.wikipedia.org/wiki/SQLite"}),
        )
        .await
        .expect("wikipedia")
        .content;
    assert_eq!(wiki["status"], 200);
    assert!(
        wiki["title"].as_str().is_some_and(|t| t.contains("SQLite")),
        "{}",
        wiki["title"]
    );
    let text = wiki["text"].as_str().expect("text");
    assert!(
        text.contains("Hipp") && !text.contains("<script"),
        "{}",
        &text[..text.len().min(400)]
    );
    let rust = fetch
        .run(
            &ctx,
            serde_json::json!({"url": "https://www.rust-lang.org/"}),
        )
        .await
        .expect("rust-lang")
        .content;
    assert_eq!(
        (rust["status"].clone(), rust["location"].clone()),
        (
            serde_json::json!(301),
            serde_json::json!("https://rust-lang.org/")
        )
    );
}
