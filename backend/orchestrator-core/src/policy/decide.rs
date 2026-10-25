//! The pure verdict for one call: the strictest decision its facts allow, and why.
//! No I/O, no clock, no locks, so the same facts always give the same answer.

use std::cmp::Reverse;

use super::{Decision, Effect, Facts, Rule};
use crate::tools::Target;

/// Returns the strictest decision and every rule that applied, the verdict's rule first.
pub fn decide(facts: &Facts) -> (Decision, Vec<Rule>) {
    let mut applied: Vec<(Decision, Rule)> = Vec::new();

    // 1. The agent's own setting shapes the call only when it is stricter than allow.
    if facts.own > Decision::Allow {
        applied.push((facts.own, Rule::AgentPolicy));
    }

    // 2. A fetch to a host off the allowlist asks at least.
    if let Target::Host { host, .. } = &facts.action.target
        && !host_allowed(host, facts.allowlist)
    {
        let rule = Rule::OffAllowlist { host: host.clone() };
        applied.push((Decision::Ask, rule));
    }

    // 3. Once tainted, a write or egress the agent allows asks instead.
    if facts.own == Decision::Allow
        && raised_by_taint(facts.effect)
        && let Some(source) = facts.taint.first()
    {
        let rule = Rule::Taint {
            source: source.clone(),
        };
        applied.push((Decision::Ask, rule));
    }

    // 4. External always asks. Validation already forbids allow; this holds even if it fails.
    if facts.own == Decision::Allow && facts.effect == Effect::External {
        applied.push((Decision::Ask, Rule::ExternalEffect));
    }

    let decision = applied
        .iter()
        .map(|(d, _)| *d)
        .fold(facts.own, Decision::max);

    // A stable sort keeps evaluation order among equals, so the first rule set the verdict.
    applied.sort_by_key(|(decision, _)| Reverse(*decision));
    (
        decision,
        applied.into_iter().map(|(_, rule)| rule).collect(),
    )
}

/// Every arm is listed, so a new `Effect` variant fails to compile until it is placed here.
fn raised_by_taint(effect: Effect) -> bool {
    match effect {
        Effect::Write | Effect::Egress => true,
        Effect::Control | Effect::Read | Effect::Memory | Effect::External => false,
    }
}

/// An entry matches its own host and any subdomain of it, never a look-alike such as
/// `badexample.com` for `example.com`. Case and a trailing dot are ignored on both sides.
fn host_allowed(host: &str, allowlist: &[String]) -> bool {
    let host = normalize(host);
    allowlist.iter().any(|entry| {
        let entry = normalize(entry);
        !entry.is_empty()
            && host
                .strip_suffix(entry.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.ends_with('.'))
    })
}

fn normalize(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::TaintSource;
    use crate::tools::{Action, MemoryScope, NoteKey};

    const DECISIONS: [Decision; 3] = [Decision::Allow, Decision::Ask, Decision::Deny];
    const EFFECTS: [Effect; 6] = [
        Effect::Control,
        Effect::Read,
        Effect::Memory,
        Effect::Write,
        Effect::Egress,
        Effect::External,
    ];

    fn host(name: &str) -> Target {
        Target::Host {
            url: format!("https://{name}/"),
            host: name.to_string(),
        }
    }

    fn file() -> Target {
        Target::File {
            path: "notes.md".to_string(),
        }
    }

    fn note() -> Target {
        Target::Note {
            scope: MemoryScope::Shared,
            key: NoteKey::new("k").expect("valid key"),
        }
    }

    fn recipient() -> Target {
        Target::Recipient {
            channel: "email".to_string(),
            address: "a@example.com".to_string(),
        }
    }

    fn inbox() -> TaintSource {
        TaintSource::Inbox {
            message_id: "m1".to_string(),
        }
    }

    fn run(
        own: Decision,
        effect: Effect,
        target: Target,
        tainted: bool,
        allowlist: &[String],
    ) -> (Decision, Vec<Rule>) {
        let action = Action {
            verb: "test".to_string(),
            target,
        };
        let taint = if tainted { vec![inbox()] } else { vec![] };
        decide(&Facts {
            own,
            effect,
            action: &action,
            taint,
            allowlist,
        })
    }

    #[test]
    fn decision_order_is_allow_ask_deny() {
        assert!(Decision::Allow < Decision::Ask && Decision::Ask < Decision::Deny);
    }

    #[test]
    fn decide_table() {
        use Decision::{Allow, Ask, Deny};
        let off = |h: &str| Rule::OffAllowlist {
            host: h.to_string(),
        };
        let taint = Rule::Taint { source: inbox() };
        let policy = Rule::AgentPolicy;

        // (own, effect, target, tainted, expected decision, expected rules)
        let rows = vec![
            (Allow, Effect::Read, file(), false, Allow, vec![]),
            (Ask, Effect::Write, file(), false, Ask, vec![policy.clone()]),
            (
                Deny,
                Effect::Write,
                file(),
                true,
                Deny,
                vec![policy.clone()],
            ),
            (Ask, Effect::Write, file(), true, Ask, vec![policy.clone()]),
            (Allow, Effect::Write, file(), true, Ask, vec![taint.clone()]),
            (Allow, Effect::Read, file(), true, Allow, vec![]),
            (Allow, Effect::Control, file(), true, Allow, vec![]),
            (Allow, Effect::Memory, note(), true, Allow, vec![]),
            (
                Allow,
                Effect::Egress,
                host("api.example.com"),
                false,
                Allow,
                vec![],
            ),
            (
                Allow,
                Effect::Egress,
                host("Example.COM."),
                false,
                Allow,
                vec![],
            ),
            (
                Allow,
                Effect::Egress,
                host("badexample.com"),
                false,
                Ask,
                vec![off("badexample.com")],
            ),
            (
                Allow,
                Effect::Egress,
                host("evil.test"),
                true,
                Ask,
                vec![off("evil.test"), taint.clone()],
            ),
            (
                Allow,
                Effect::Egress,
                host("api.example.com"),
                true,
                Ask,
                vec![taint.clone()],
            ),
            (
                Deny,
                Effect::Egress,
                host("evil.test"),
                false,
                Deny,
                vec![policy.clone(), off("evil.test")],
            ),
            (
                Allow,
                Effect::External,
                recipient(),
                false,
                Ask,
                vec![Rule::ExternalEffect],
            ),
            (
                Ask,
                Effect::External,
                recipient(),
                false,
                Ask,
                vec![policy.clone()],
            ),
        ];

        let allowlist = vec!["example.com".to_string()];
        for (i, (own, effect, target, tainted, want, want_rules)) in rows.into_iter().enumerate() {
            let got = run(own, effect, target, tainted, &allowlist);
            assert_eq!(got, (want, want_rules), "row {i}");
        }
    }

    #[test]
    fn empty_allowlist_asks_for_every_host() {
        let (decision, rules) = run(
            Decision::Allow,
            Effect::Egress,
            host("example.com"),
            false,
            &[],
        );
        assert_eq!(decision, Decision::Ask);
        assert_eq!(
            rules,
            vec![Rule::OffAllowlist {
                host: "example.com".to_string()
            }]
        );
    }

    #[test]
    fn decide_is_never_looser_than_the_setting_and_taint_never_loosens() {
        let allowlist = vec!["example.com".to_string()];
        for own in DECISIONS {
            for effect in EFFECTS {
                for target in [file(), host("example.com"), host("evil.test")] {
                    let (clean, _) = run(own, effect, target.clone(), false, &allowlist);
                    let (tainted, _) = run(own, effect, target, true, &allowlist);
                    assert!(clean >= own, "{own:?} {effect:?} loosened to {clean:?}");
                    assert!(tainted >= clean, "{own:?} {effect:?}: taint loosened");
                    if effect == Effect::External {
                        assert!(clean >= Decision::Ask, "{own:?}: external was allowed");
                    }
                }
            }
        }
    }
}
