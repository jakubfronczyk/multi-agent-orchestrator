//! The run loop (§7.1): budget, cancellation, step limit, one model call per step.
//! Every exit ends in exactly one `run_finished`; a failed emit ends the run.

use std::sync::Arc;

use futures_util::{StreamExt, stream};

use crate::agents::AgentSpec;
use crate::llm::{CompletionRequest, CompletionResponse, Item, LlmError, ScriptKey, ToolChoice};
use crate::runs::{FailReason, RunEvent, RunOutcome};
use crate::store::{RunRef, StoreError};
use crate::tools::ToolSpec;

use super::{Inner, Task, gateway};

const MAX_PARALLEL_CALLS: usize = 4;

pub(super) struct RunCtx {
    pub at: RunRef,
    pub agent: Arc<AgentSpec>,
    pub task_text: String,
    pub scenario: Option<String>,
    pub task: Arc<Task>,
}

pub(super) async fn run_agent(inner: Arc<Inner>, ctx: RunCtx) {
    let outcome = match run_loop(&inner, &ctx).await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(%error, run_id = ?ctx.at.run_id, "storage failed during the run");
            failed(FailReason::Internal, "storage failed during the run")
        }
    };
    if let Err(error) = inner
        .store
        .emit(ctx.at, RunEvent::RunFinished { outcome })
        .await
    {
        tracing::error!(%error, run_id = ?ctx.at.run_id, "could not record the run's end");
    }
}

async fn run_loop(inner: &Inner, ctx: &RunCtx) -> Result<RunOutcome, StoreError> {
    let cancel = ctx.task.cancel_token();
    let mut transcript = vec![Item::User {
        text: ctx.task_text.clone(),
    }];
    let offered: Vec<ToolSpec> = ctx
        .agent
        .tools()
        .keys()
        .filter_map(|name| inner.tools.get(*name).map(|tool| tool.spec().clone()))
        .collect();
    let mut step = 0;
    loop {
        if ctx.task.spent() >= ctx.task.budget() {
            cancel.cancel();
            return Ok(failed(
                FailReason::BudgetExceeded,
                "the task's budget is spent",
            ));
        }
        if cancel.is_cancelled() {
            return Ok(RunOutcome::Cancelled);
        }
        let last = step >= ctx.agent.max_steps();

        emit(inner, ctx, RunEvent::LlmRequested { step }).await?;
        let request = CompletionRequest {
            model: ctx.agent.model(),
            reasoning_effort: ctx.agent.reasoning_effort(),
            instructions: ctx.agent.system_prompt().to_string(),
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
        emit(inner, ctx, responded.clone()).await?;
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

/// Retries only errors `LlmError::is_retryable` allows, once per configured delay.
async fn complete_with_retries(
    inner: &Inner,
    ctx: &RunCtx,
    step: u32,
    request: CompletionRequest,
) -> Result<Attempt, StoreError> {
    let cancel = ctx.task.cancel_token();
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
            Some(delay) if error.is_retryable() => *delay,
            Some(_) | None => return Ok(Attempt::Failed(error)),
        };
        attempt += 1;
        let message = error.to_string();
        emit(
            inner,
            ctx,
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

async fn emit(inner: &Inner, ctx: &RunCtx, event: RunEvent) -> Result<(), StoreError> {
    inner.store.emit(ctx.at, event).await.map(|_| ())
}

fn failed(reason: FailReason, message: &str) -> RunOutcome {
    RunOutcome::Failed {
        reason,
        message: message.to_string(),
    }
}
