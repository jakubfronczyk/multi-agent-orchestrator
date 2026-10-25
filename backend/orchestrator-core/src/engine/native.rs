//! The two tools the engine runs itself: `delegate` starts a child run, `plan` records steps.
//! Their arguments are parsed and checked like any tool's before a decision is made.

use std::sync::Arc;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::{AgentSpec, Slug};
use crate::ids::{RunId, ToolCallId};
use crate::runs::{CallStatus, RunEvent, RunOutcome};
use crate::store::{RunRef, StoreError};
use crate::tools::{
    Action, PlanStep, Target, ToolError, ToolName, ToolOutput, ToolSpec, parse_args, schema_for,
};

use super::gateway::Ended;
use super::run::{self, RunCtx, spawn_run};
use super::{Inner, envelope, prompts};

const TASK_CHARS: usize = 4000;
const CONTEXT_CHARS: usize = 8000;
const EXPECTED_OUTPUT_CHARS: usize = 500;
const PLAN_STEPS: usize = 8;
const GOAL_CHARS: usize = 300;
const PLAN_TIMEOUT: Duration = Duration::from_secs(1);

/// Checked delegation arguments; `delegate` runs from these, never from the raw JSON.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DelegateArgs {
    /// The teammate to hand this step to.
    agent: String,
    /// What the teammate should do. It sees nothing else of your conversation.
    task: String,
    /// Facts the teammate needs to do the task.
    #[serde(default)]
    context: Option<String>,
    /// What the answer should look like.
    #[serde(default)]
    expected_output: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanArgs {
    /// One to eight steps, in order.
    steps: Vec<PlanStepArgs>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanStepArgs {
    /// A teammate's slug, or "self".
    agent: String,
    /// What this step achieves, in one to 300 characters.
    goal: String,
}

/// The `agent` enum is the caller's live targets, so the schema is built per agent.
pub fn delegate_spec(targets: &[Arc<AgentSpec>]) -> ToolSpec {
    let mut schema = schema_for::<DelegateArgs>();
    let slugs: Vec<&str> = targets.iter().map(|t| t.slug().as_str()).collect();
    schema["properties"]["agent"]["enum"] = json!(slugs);
    ToolSpec {
        name: ToolName::Delegate,
        description: prompts::delegate_description(targets),
        schema,
        timeout: None,
    }
}

pub fn plan_spec() -> ToolSpec {
    ToolSpec {
        name: ToolName::Plan,
        description: prompts::plan_description(),
        schema: schema_for::<PlanArgs>(),
        timeout: Some(PLAN_TIMEOUT),
    }
}

pub(super) fn check_delegate(
    args: &Value,
    targets: &[Slug],
) -> Result<(Action, DelegateArgs), ToolError> {
    let args = parse_args::<DelegateArgs>(args)?;
    let slug = target(&args.agent, targets)?;
    within("task", &args.task, 1, TASK_CHARS)?;
    within(
        "context",
        args.context.as_deref().unwrap_or(""),
        0,
        CONTEXT_CHARS,
    )?;
    within(
        "expected_output",
        args.expected_output.as_deref().unwrap_or(""),
        0,
        EXPECTED_OUTPUT_CHARS,
    )?;
    let action = Action {
        verb: "delegate".to_string(),
        target: Target::Agent { slug },
    };
    Ok((action, args))
}

pub(super) fn check_plan(
    args: &Value,
    targets: &[Slug],
) -> Result<(Action, Vec<PlanStep>), ToolError> {
    let args = parse_args::<PlanArgs>(args)?;
    if args.steps.is_empty() || args.steps.len() > PLAN_STEPS {
        return Err(ToolError::InvalidArgs(format!(
            "steps: give 1 to {PLAN_STEPS} steps"
        )));
    }
    let mut steps = Vec::with_capacity(args.steps.len());
    for step in args.steps {
        if step.agent != "self" {
            target(&step.agent, targets)?;
        }
        within("goal", &step.goal, 1, GOAL_CHARS)?;
        steps.push(PlanStep {
            agent: step.agent,
            goal: step.goal,
        });
    }
    let action = Action {
        verb: "plan".to_string(),
        target: Target::Plan {
            steps: steps.clone(),
        },
    };
    Ok((action, steps))
}

pub(super) async fn plan(
    inner: &Inner,
    ctx: &RunCtx,
    steps: Vec<PlanStep>,
) -> Result<Ended, StoreError> {
    let count = steps.len();
    inner.emit(ctx.at, RunEvent::PlanRecorded { steps }).await?;
    Ok(Ended::Ok(ToolOutput {
        content: json!({ "recorded_steps": count }),
        taint: Vec::new(),
    }))
}

/// Starts the child as its own spawned run and waits for it. A failed or cancelled child
/// is an error result; the caller continues either way.
pub(super) async fn delegate(
    inner: &Arc<Inner>,
    ctx: &RunCtx,
    call_id: ToolCallId,
    args: DelegateArgs,
) -> Result<Ended, StoreError> {
    let Some(target) = ctx.task.teammate(&args.agent) else {
        return Ok(Ended::failed(
            CallStatus::Failed,
            "that teammate is no longer on the team",
            None,
        ));
    };
    // The run limit is shared by parallel calls, so a slot is claimed only as the child starts.
    // A limit is not a policy refusal: no rule applied, so the call ends as failed.
    if !ctx.task.try_start_run(inner.config.max_runs) {
        return Ok(Ended::failed(
            CallStatus::Failed,
            "run limit reached; do not retry",
            None,
        ));
    }

    let text = prompts::child_message(
        &args.task,
        args.context.as_deref(),
        args.expected_output.as_deref(),
    );
    // Rule 5: once the task is tainted, the child receives its task as data; see `run_loop`.
    let task_sources = ctx.task.taint_sources();
    let child = RunRef {
        root_run_id: ctx.at.root_run_id,
        run_id: RunId::new(),
    };
    let started = RunEvent::RunStarted {
        agent_id: target.id(),
        parent_run_id: Some(ctx.at.run_id),
        parent_call_id: Some(call_id),
        task: text.clone(),
        task_sources: task_sources.clone(),
        title: None,
        budget: None,
        scenario: None,
        agent_version: Some(target.version()),
        draft_only: ctx.task.draft_only(),
    };
    inner.emit(child, started).await?;

    let mut agent_path = ctx.agent_path.clone();
    agent_path.push(target.slug().clone());
    let child_ctx = RunCtx {
        at: child,
        agent: Arc::clone(&target),
        task_text: text,
        task_sources,
        scenario: ctx.scenario.clone(),
        task: Arc::clone(&ctx.task),
        depth: ctx.depth + 1,
        cancel: ctx.cancel.child_token(),
        caller: Some(ctx.agent.name().to_string()),
        agent_path,
        history: Vec::new(),
    };
    let child_cancel = child_ctx.cancel.clone();
    let outcome = match spawn_run(Arc::clone(inner), child_ctx).await {
        Ok(outcome) => outcome,
        Err(join_error) => {
            // As the root supervisor does: stop anything the child started before recording it.
            child_cancel.cancel();
            let outcome = run::stopped(join_error);
            inner
                .emit(
                    child,
                    RunEvent::RunFinished {
                        outcome: outcome.clone(),
                    },
                )
                .await?;
            outcome
        }
    };

    Ok(match outcome {
        RunOutcome::Completed { output, truncated } => Ended::Child {
            agent: target.slug().clone(),
            run_id: child.run_id,
            truncated,
            output,
            untrusted: envelope::source_label(&ctx.task.taint_sources()),
        },
        RunOutcome::Failed { reason, message } => {
            let message = if ctx.task.is_tainted() {
                prompts::failure_text(reason)
            } else {
                &message
            };
            Ended::failed(CallStatus::ChildFailed, message, None)
        }
        RunOutcome::Cancelled | RunOutcome::Interrupted => Ended::failed(
            CallStatus::ChildCancelled,
            "the teammate's run was cancelled",
            None,
        ),
    })
}

fn target(agent: &str, targets: &[Slug]) -> Result<Slug, ToolError> {
    targets
        .iter()
        .find(|slug| slug.as_str() == agent)
        .cloned()
        .ok_or_else(|| {
            ToolError::InvalidArgs(format!("agent: {agent} is not one of your teammates"))
        })
}

fn within(field: &str, value: &str, min: usize, max: usize) -> Result<(), ToolError> {
    let chars = value.chars().count();
    if chars < min || chars > max {
        return Err(ToolError::InvalidArgs(format!(
            "{field}: use {min} to {max} characters"
        )));
    }
    Ok(())
}
