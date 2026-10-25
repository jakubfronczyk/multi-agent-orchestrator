//! The run loop (§7.1): budget, cancellation, step limit, one model call per step.
//! Every exit ends in exactly one `run_finished`; a failed emit ends the run.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::agents::{AgentSpec, Slug};
use crate::llm::{CompletionRequest, CompletionResponse, Item, LlmError, ScriptKey, ToolChoice};
use crate::policy::TaintSource;
use crate::runs::{FailReason, RunEvent, RunOutcome};
use crate::store::{RunRef, StoreError};

use super::{Inner, Task, envelope, gateway, prompts};

const MAX_PARALLEL_CALLS: usize = 4;

pub(super) struct RunCtx {
    pub at: RunRef,
    pub agent: Arc<AgentSpec>,
    /// The task text this run was given, plain: the user's for a root, the caller's for a child.
    pub task_text: String,
    /// The task's taint when this run was delegated; set means the model gets `task_text` as data.
    pub task_sources: Vec<TaintSource>,
    pub scenario: Option<String>,
    pub task: Arc<Task>,
    /// 0 for a root run, one more for each delegation.
    pub depth: u32,
    /// The task's token for a root, a child of the caller's for a delegated run.
    pub cancel: CancellationToken,
    /// The calling agent's name; `None` for a root run.
    pub caller: Option<String>,
    /// Slugs from the root agent to this one, for custody.
    pub agent_path: Vec<Slug>,
}

/// Every run is its own spawned task. The future is boxed because a run can start a child
/// run, and a future type cannot contain itself.
pub(super) fn spawn_run(inner: Arc<Inner>, ctx: RunCtx) -> JoinHandle<RunOutcome> {
    let run: std::pin::Pin<Box<dyn Future<Output = RunOutcome> + Send>> =
        Box::pin(run_agent(inner, ctx));
    tokio::spawn(run)
}

/// Emits the run's one `run_finished` and returns the outcome it recorded.
async fn run_agent(inner: Arc<Inner>, ctx: RunCtx) -> RunOutcome {
    let outcome = match run_loop(&inner, &ctx).await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(%error, run_id = ?ctx.at.run_id, "storage failed during the run");
            failed(FailReason::Internal, "storage failed during the run")
        }
    };
    let finished = RunEvent::RunFinished {
        outcome: outcome.clone(),
    };
    if let Err(error) = inner.emit(ctx.at, finished).await {
        tracing::error!(%error, run_id = ?ctx.at.run_id, "could not record the run's end");
    }
    outcome
}

async fn run_loop(inner: &Arc<Inner>, ctx: &RunCtx) -> Result<RunOutcome, StoreError> {
    // Rule 5: the wrapper exists only in what the model reads; storage and the API keep plain text.
    let text = if ctx.task_sources.is_empty() {
        ctx.task_text.clone()
    } else {
        prompts::untrusted(&envelope::source_label(&ctx.task_sources), &ctx.task_text)
    };
    let mut transcript = vec![Item::User { text }];
    let offered = gateway::offered_specs(inner, ctx);
    let instructions = prompts::instructions(&ctx.agent, ctx.caller.as_deref());
    let mut step = 0;
    loop {
        if ctx.task.spent() >= ctx.task.budget() {
            ctx.task.cancel_token().cancel();
            return Ok(failed(
                FailReason::BudgetExceeded,
                "the task's budget is spent",
            ));
        }
        if ctx.cancel.is_cancelled() {
            return Ok(RunOutcome::Cancelled);
        }
        let last = step >= ctx.agent.max_steps();

        inner.emit(ctx.at, RunEvent::LlmRequested { step }).await?;
        let request = CompletionRequest {
            model: ctx.agent.model(),
            reasoning_effort: ctx.agent.reasoning_effort(),
            instructions: instructions.clone(),
            transcript: transcript.clone(),
            tools: offered.clone(),
            tool_choice: if last {
                ToolChoice::None
            } else {
                ToolChoice::Auto
            },
            script: ScriptKey {
                root_run_id: ctx.at.root_run_id,
                agent: ctx.agent.slug().clone(),
                step,
                scenario: ctx.scenario.clone(),
            },
        };
        let response = match complete_with_retries(inner, ctx, step, request).await? {
            Attempt::Done(response) => response,
            Attempt::Cancelled => return Ok(RunOutcome::Cancelled),
            Attempt::Failed(error) => return Ok(failed(FailReason::LlmError, &error.to_string())),
        };

        let responded = RunEvent::LlmResponded {
            step,
            text: response.text.clone(),
            usage: response.usage,
            cost: response.cost_micros,
        };
        inner.emit(ctx.at, responded.clone()).await?;
        ctx.task.apply(&responded);
        transcript.extend(response.raw.into_iter().map(Item::Provider));

        let output = response.text.unwrap_or_default();
        if last {
            return Ok(RunOutcome::Completed {
                output,
                truncated: true,
            });
        }
        if response.calls.is_empty() {
            return Ok(RunOutcome::Completed {
                output,
                truncated: false,
            });
        }

        // Up to MAX_PARALLEL_CALLS run at once, each decided when it starts; `buffered`
        // yields results in call order, so outputs line up with the calls.
        let outputs: Vec<Result<String, StoreError>> = stream::iter(response.calls.clone())
            .map(|call| async move { gateway::execute(inner, ctx, &call).await })
            .buffered(MAX_PARALLEL_CALLS)
            .collect()
            .await;
        for (call, output) in response.calls.iter().zip(outputs) {
            transcript.push(Item::ToolOutput {
                call_id: call.call_id.clone(),
                output: output?,
            });
        }
        step += 1;
    }
}

enum Attempt {
    Done(CompletionResponse),
    Cancelled,
    Failed(LlmError),
}

/// Retries only errors `LlmError::is_retryable` allows, once per configured base delay.
/// Equal jitter waits half the base plus up to half more, so concurrent runs spread out and none retries at once.
async fn complete_with_retries(
    inner: &Inner,
    ctx: &RunCtx,
    step: u32,
    request: CompletionRequest,
) -> Result<Attempt, StoreError> {
    let cancel = &ctx.cancel;
    let mut delays = inner.config.retry_backoff.iter();
    let mut attempt = 0;
    loop {
        let result = tokio::select! {
            () = cancel.cancelled() => return Ok(Attempt::Cancelled),
            result = complete_in_slot(inner, request.clone()) => result,
        };
        let error = match result {
            Ok(response) => return Ok(Attempt::Done(response)),
            Err(error) => error,
        };
        let delay = match delays.next() {
            Some(base) if error.is_retryable() => {
                *base / 2 + rand::random_range(Duration::ZERO..=*base / 2)
            }
            Some(_) | None => return Ok(Attempt::Failed(error)),
        };
        attempt += 1;
        let message = error.to_string();
        inner
            .emit(
                ctx.at,
                RunEvent::LlmRetrying {
                    step,
                    attempt,
                    message,
                },
            )
            .await?;
        tokio::select! {
            () = cancel.cancelled() => return Ok(Attempt::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// The permit is held only while the provider works, so waiting runs never block a slot.
/// The timeout is the engine's, so a provider without its own still cannot stall a run.
async fn complete_in_slot(
    inner: &Inner,
    request: CompletionRequest,
) -> Result<CompletionResponse, LlmError> {
    let _permit = inner
        .llm_slots
        .acquire()
        .await
        .map_err(|_| LlmError::Internal("the model call limiter is closed".to_string()))?;
    tokio::time::timeout(inner.config.llm_timeout, inner.llm.complete(request))
        .await
        .unwrap_or(Err(LlmError::Timeout))
}

fn failed(reason: FailReason, message: &str) -> RunOutcome {
    RunOutcome::Failed {
        reason,
        message: message.to_string(),
    }
}
