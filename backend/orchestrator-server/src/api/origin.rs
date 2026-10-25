//! Refuses what a browser sends on behalf of another site. The server binds to loopback only,
//! so a Host that is not a loopback name is DNS rebinding, and a state-changing request whose
//! Origin is not loopback came from another site's page.

use axum::extract::Request;
use axum::http::Method;
use axum::http::header::{HOST, ORIGIN};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::ApiError;

const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// Any port is accepted, so the Vite dev proxy, which keeps the browser's Host, passes.
pub(super) async fn loopback_only(request: Request, next: Next) -> Response {
    if from_this_app(&request) {
        next.run(request).await
    } else {
        ApiError::forbidden().into_response()
    }
}

fn from_this_app(request: &Request) -> bool {
    let header = |name| request.headers().get(name).and_then(|v| v.to_str().ok());
    let host_ok = header(HOST).is_some_and(is_loopback);
    // A request without Origin is not a browser's cross-site request; GET and HEAD change nothing.
    let origin_ok = matches!(*request.method(), Method::GET | Method::HEAD)
        || header(ORIGIN).is_none_or(|origin| {
            origin
                .strip_prefix("http://")
                .or_else(|| origin.strip_prefix("https://"))
                .is_some_and(is_loopback)
        });
    host_ok && origin_ok
}

/// `authority` is `host[:port]`, with an IPv6 host in brackets.
fn is_loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or("", |(host, _)| host),
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    LOOPBACK.iter().any(|name| host.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::is_loopback;

    #[test]
    fn only_loopback_names_pass() {
        for ok in [
            "127.0.0.1:8787",
            "localhost:5173",
            "LOCALHOST",
            "[::1]:8787",
        ] {
            assert!(is_loopback(ok), "{ok}");
        }
        for bad in [
            "evil.test:8787",
            "127.0.0.1.evil.test",
            "localhost.evil.test:80",
            "[::2]:8787",
            "",
            "null",
        ] {
            assert!(!is_loopback(bad), "{bad}");
        }
    }
}
