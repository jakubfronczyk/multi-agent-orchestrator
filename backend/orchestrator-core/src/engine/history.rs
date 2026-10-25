//! A task read back from its event log, so a follow-up continues it (ADR 0002): the log, not
//! memory, says what the task read, spent and had refused.

use std::collections::HashSet;

use crate::ids::{AgentId, RunId};
use crate::llm::Item;
use crate::money::Micros;
use crate::policy::TaintSource;
use crate::runs::{RunEvent, RunOutcome};

use super::{Task, envelope, prompts};

/// One top-level run of the task: the root or a follow-up.
struct Turn {
    text: String,
    outcome: Option<RunOutcome>,
}

pub(super) struct History {
    /// The root run's agent; a follow-up runs it again.
    pub root_agent: AgentId,
    pub budget: Micros,
    /// Some run started and has not finished.
    pub open: bool,
    turns: Vec<Turn>,
}

impl History {
    /// `None` when the events hold no root run for `root`.
    pub fn read(root: RunId, events: &[(RunId, RunEvent)]) -> Option<History> {
        let mut root_start = None;
        let mut turns: Vec<(RunId, Turn)> = Vec::new();
        let mut unfinished = HashSet::new();
        for (run, event) in events {
            match event {
                RunEvent::RunStarted {
                    agent_id,
                    parent_run_id,
                    task,
                    budget,
                    ..
                } => {
                    unfinished.insert(*run);
                    if *run == root {
                        root_start = Some((*agent_id, *budget));
                    }
                    if parent_run_id.is_none() {
                        let text = task.clone();
                        turns.push((
                            *run,
                            Turn {
                                text,
                                outcome: None,
                            },
                        ));
                    }
                }
                RunEvent::RunFinished { outcome } => {
                    unfinished.remove(run);
                    if let Some((_, turn)) = turns.iter_mut().find(|(id, _)| id == run) {
                        turn.outcome = Some(outcome.clone());
                    }
                }
                _ => {}
            }
        }
        let (root_agent, budget) = root_start?;
        Some(History {
            root_agent,
            // The root always records its budget; a missing one reads as nothing left.
            budget: budget.unwrap_or(Micros(0)),
            open: !unfinished.is_empty(),
            turns: turns.into_iter().map(|(_, turn)| turn).collect(),
        })
    }

    /// Each earlier turn as the user's text and the Lead's answer. In a tainted task each
    /// answer is wrapped as untrusted, as any derived text is.
    pub fn items(&self, taint: &[TaintSource]) -> Vec<Item> {
        let source = (!taint.is_empty()).then(|| envelope::source_label(taint));
        let mut items = Vec::with_capacity(self.turns.len() * 2);
        for turn in &self.turns {
            items.push(Item::User {
                text: turn.text.clone(),
            });
            let text = match &turn.outcome {
                Some(RunOutcome::Completed { output, .. }) => match &source {
                    Some(source) => prompts::untrusted(source, output),
                    None => output.clone(),
                },
                Some(RunOutcome::Cancelled) => "Earlier turn: cancelled by the user".to_string(),
                Some(RunOutcome::Failed { message, .. }) => {
                    format!("Earlier turn failed: {message}")
                }
                Some(RunOutcome::Interrupted) => {
                    "Earlier turn failed: interrupted by a server restart".to_string()
                }
                None => continue,
            };
            items.push(Item::Assistant { text });
        }
        items
    }
}

/// Rebuilds the task's taint, spending and refused calls by replaying its log.
pub(super) fn replay(task: &Task, events: &[(RunId, RunEvent)]) {
    for (_, event) in events {
        match event {
            RunEvent::TaskTainted { source } => {
                task.claim_taint(source);
            }
            other => task.apply(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::FailReason;

    fn started(task: &str, parent: Option<RunId>) -> RunEvent {
        RunEvent::RunStarted {
            agent_id: AgentId::new(),
            parent_run_id: parent,
            parent_call_id: None,
            task: task.to_string(),
            task_sources: Vec::new(),
            title: None,
            budget: Some(Micros(9)),
            scenario: None,
            agent_version: None,
        }
    }

    fn finished(outcome: RunOutcome) -> RunEvent {
        RunEvent::RunFinished { outcome }
    }

    #[test]
    fn earlier_turns_become_user_text_and_one_answer_each() {
        let (root, child, second, third) = (RunId::new(), RunId::new(), RunId::new(), RunId::new());
        let events = vec![
            (root, started("first", None)),
            (child, started("a step", Some(root))),
            (child, finished(RunOutcome::Cancelled)),
            (
                root,
                finished(RunOutcome::Completed {
                    output: "done".to_string(),
                    truncated: false,
                }),
            ),
            (second, started("second", None)),
            (second, finished(RunOutcome::Cancelled)),
            (third, started("third", None)),
            (
                third,
                finished(RunOutcome::Failed {
                    reason: FailReason::BudgetExceeded,
                    message: "the task's budget is spent".to_string(),
                }),
            ),
        ];
        let history = History::read(root, &events).expect("a root");
        assert!(!history.open);
        assert_eq!(history.budget, Micros(9));

        let texts: Vec<String> = history
            .items(&[])
            .into_iter()
            .map(|item| match item {
                Item::User { text } => format!("user: {text}"),
                Item::Assistant { text } => format!("lead: {text}"),
                Item::Provider(_) | Item::ToolOutput { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            texts,
            [
                "user: first",
                "lead: done",
                "user: second",
                "lead: Earlier turn: cancelled by the user",
                "user: third",
                "lead: Earlier turn failed: the task's budget is spent",
            ]
        );
    }

    #[test]
    fn a_tainted_task_wraps_each_answer_and_an_open_run_is_seen() {
        let root = RunId::new();
        let mut events = vec![
            (root, started("first", None)),
            (
                root,
                finished(RunOutcome::Completed {
                    output: "done".to_string(),
                    truncated: false,
                }),
            ),
        ];
        let web = TaintSource::Web {
            url: "https://example.com".to_string(),
        };
        let items = History::read(root, &events)
            .expect("a root")
            .items(std::slice::from_ref(&web));
        assert_eq!(
            items[1],
            Item::Assistant {
                text: prompts::untrusted("https://example.com", "done")
            }
        );

        events.push((RunId::new(), started("next", None)));
        assert!(History::read(root, &events).expect("a root").open);
        assert!(History::read(RunId::new(), &events).is_none());
    }
}
