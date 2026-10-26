//! What a turn saved and sent, written from its finished calls rather than by a model, so the
//! answer's account of saves and sends is true however the model worded the rest.

use crate::runs::CallStatus;
use crate::tools::{Target, ToolName};

use super::task::Done;

/// The line every finished answer ends with; the web shows its own only for answers without it.
pub const PREFIX: &str = "From the record:";

/// One sentence of what was saved, noted and sent, then what was attempted and did not happen.
pub fn summary(actions: &[Done]) -> String {
    let mut done = Vec::new();
    let mut missed = Vec::new();
    for action in actions {
        let what = describe(action.tool, &action.target);
        match action.status {
            CallStatus::Ok => done.push(what.done),
            status => missed.push(format!("{} ({})", what.attempted, why(status))),
        }
    }
    if done.is_empty() && missed.is_empty() {
        return format!("{PREFIX} nothing was saved or sent.");
    }
    let mut sentence = PREFIX.to_string();
    if done.is_empty() {
        sentence.push_str(" nothing was saved or sent.");
    } else {
        sentence.push(' ');
        sentence.push_str(&done.join("; "));
        sentence.push('.');
    }
    if !missed.is_empty() {
        sentence.push_str(" Not done: ");
        sentence.push_str(&missed.join("; "));
        sentence.push('.');
    }
    sentence
}

struct Described {
    done: String,
    attempted: String,
}

fn describe(tool: ToolName, target: &Target) -> Described {
    match (tool, target) {
        (ToolName::FileWrite, Target::File { path }) => Described {
            done: format!("saved {path}"),
            attempted: format!("saving {path}"),
        },
        (ToolName::MemoryWrite, Target::Note { key, .. }) => Described {
            done: format!("saved the note {}", key.as_str()),
            attempted: format!("saving the note {}", key.as_str()),
        },
        (
            ToolName::SendMessage,
            Target::Recipient {
                channel,
                address,
                cc,
            },
        ) => {
            let kind = if channel == "email" {
                "an email"
            } else {
                "a message"
            };
            let copies = if cc.is_empty() {
                String::new()
            } else {
                format!(", copying {}", cc.join(", "))
            };
            Described {
                done: format!("sent {kind} to {address}{copies}"),
                attempted: format!("sending {kind} to {address}{copies}"),
            }
        }
        _ => Described {
            done: format!("ran {tool:?}"),
            attempted: format!("{tool:?}"),
        },
    }
}

fn why(status: CallStatus) -> &'static str {
    match status {
        CallStatus::DeniedHuman => "you declined it",
        CallStatus::Expired => "nobody answered in time",
        CallStatus::Refused | CallStatus::NotOffered | CallStatus::InvalidArgs => {
            "refused by the rules"
        }
        CallStatus::Cancelled | CallStatus::Interrupted => "the task stopped first",
        CallStatus::Ok
        | CallStatus::Timeout
        | CallStatus::Failed
        | CallStatus::Internal
        | CallStatus::ChildFailed
        | CallStatus::ChildCancelled => "it failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{MemoryScope, NoteKey};

    fn send(address: &str, cc: &[&str], status: CallStatus) -> Done {
        Done {
            tool: ToolName::SendMessage,
            status,
            target: Target::Recipient {
                channel: "email".to_string(),
                address: address.to_string(),
                cc: cc.iter().map(|c| c.to_string()).collect(),
            },
        }
    }

    #[test]
    fn the_record_names_what_happened_and_what_did_not() {
        assert_eq!(summary(&[]), "From the record: nothing was saved or sent.");
        let actions = [
            Done {
                tool: ToolName::FileWrite,
                status: CallStatus::Ok,
                target: Target::File {
                    path: "decisions/AR-1042.md".to_string(),
                },
            },
            Done {
                tool: ToolName::MemoryWrite,
                status: CallStatus::Ok,
                target: Target::Note {
                    scope: MemoryScope::Shared,
                    key: NoteKey::new("finance/cfo").expect("key"),
                },
            },
            send("anna@example.com", &["rolf@example.com"], CallStatus::Ok),
            send("vendor@evil.test", &[], CallStatus::DeniedHuman),
        ];
        assert_eq!(
            summary(&actions),
            "From the record: saved decisions/AR-1042.md; saved the note finance/cfo; sent an email to \
             anna@example.com, copying rolf@example.com. Not done: sending an email to vendor@evil.test \
             (you declined it)."
        );
        assert_eq!(
            summary(&[send("accounts@example.com", &[], CallStatus::Expired)]),
            "From the record: nothing was saved or sent. Not done: sending an email to \
             accounts@example.com (nobody answered in time)."
        );
    }
}
