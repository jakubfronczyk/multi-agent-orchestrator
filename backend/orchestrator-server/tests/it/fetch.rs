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
