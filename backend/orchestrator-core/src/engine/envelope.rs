//! What the model sees for each call (§5): the result envelope, untrusted wrapping and caps.
//! The gateway decides how a call ended; this module only renders it.

use serde_json::json;

use crate::policy::TaintSource;

use super::gateway::Ended;
use super::prompts;

const PREVIEW_CHARS: usize = 200;
/// The longest result text the model receives; past it, the text is cut and marked.
const RESULT_CHARS: usize = 64_000;

/// How sources appear to the model and on a card: `inbox:<id>`, the URL, `note:<key>` or
/// `file:<path>`, in read order.
pub(super) fn source_label(sources: &[TaintSource]) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let label = sources
        .iter()
        .map(|source| match source {
            TaintSource::Inbox { message_id } => format!("inbox:{message_id}"),
            TaintSource::Web { url } => url.clone(),
            TaintSource::Note { key, .. } => format!("note:{}", key.as_str()),
            TaintSource::File { path } => format!("file:{path}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(label)
}

/// `untrusted` names this call's sources when its result is outside content.
pub(super) fn render(ended: &Ended, untrusted: Option<&str>) -> String {
    match ended {
        Ended::Ok(output) => {
            let (text, cut) = capped(&output.content.to_string());
            let mut body = match untrusted {
                Some(source) => json!({
                    "ok": true,
                    "trust": "untrusted",
                    "source": source,
                    "result": prompts::untrusted(source, &text),
                }),
                None if cut => json!({"ok": true, "result": text}),
                None => json!({"ok": true, "result": output.content}),
            };
            if cut {
                body["truncated"] = json!(true);
            }
            body.to_string()
        }
        Ended::Child {
            agent,
            truncated,
            output,
            untrusted,
            calls,
            ..
        } => {
            let status = if *truncated { "truncated" } else { "completed" };
            let (text, cut) = capped(output);
            let mut body = match untrusted {
                Some(source) => json!({
                    "ok": true,
                    "agent": agent,
                    "status": status,
                    "trust": "untrusted",
                    "source": source,
                    "output": prompts::untrusted(source, &text),
                }),
                None => json!({"ok": true, "agent": agent, "status": status, "output": text}),
            };
            if cut {
                body["truncated"] = json!(true);
            }
            // Recorded by the gateway, not written by the model, so it stays outside the wrapper.
            body["calls"] = json!(calls);
            body.to_string()
        }
        Ended::Failed {
            status,
            message,
            note,
            ..
        } => {
            let retryable = status.is_retryable();
            let mut body =
                json!({"ok": false, "kind": status, "message": message, "retryable": retryable});
            if let Some(note) = note {
                body["note"] = json!(note);
            }
            body.to_string()
        }
    }
}

/// Cuts on a character boundary, so the envelope stays valid JSON.
fn capped(text: &str) -> (String, bool) {
    match text.char_indices().nth(RESULT_CHARS) {
        Some((end, _)) => (text[..end].to_string(), true),
        None => (text.to_string(), false),
    }
}

pub(super) fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}
