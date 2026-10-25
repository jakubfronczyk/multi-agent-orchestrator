//! The gateway (§7.2): every function call passes here, is decided, logged and answered.
//! Each call gets exactly one output and exactly one `tool_call_finished`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};

use crate::ids::{ApprovalId, ToolCallId};
use crate::llm::FnCall;
use crate::policy::{Decision, Rule, Trust, decide};
use crate::runs::{Answer, ApprovalOutcome, CallStatus, Custody, RunEvent};
use crate::store::StoreError;
use crate::tools::{Action, Tool, ToolCtx, ToolName, ToolOutput};

use super::Inner;
use super::approvals::Waited;
use super::run::RunCtx;

const PREVIEW_CHARS: usize = 200;

/// How a call ended, before it is rendered for the model.
enum Ended {
    Ok(ToolOutput),
    Failed {
        status: CallStatus,
        message: String,
        note: Option<String>,
    },
}

/// The output the model sees for this call. The only error is storage, which ends the run.
pub(super) async fn execute(
    inner: &Inner,
    ctx: &RunCtx,
    call: &FnCall,
) -> Result<String, StoreError> {
    let call_id = ToolCallId::new();
    let started = Instant::now();

    let offered = serde_json::from_value::<ToolName>(Value::String(call.name.clone()))
        .ok()
        .filter(|name| ctx.agent.tools().contains_key(name))
        .and_then(|name| inner.tools.get(name).map(|tool| (name, tool)));
    let Some((name, tool)) = offered else {
        let message = format!("{} is not one of your tools", call.name);
        return refuse_early(
            inner,
            ctx,
            call_id,
            call,
            Rule::NotOffered,
            CallStatus::NotOffered,
            message,
        )
        .await;
    };

    let checked = parse_args(&call.arguments).and_then(|args| {
        tool.check(&args)
            .map(|action| (args, action))
            .map_err(|e| e.to_string())
    });
    let (args, action) = match checked {
        Ok(checked) => checked,
        Err(message) => {
            return refuse_early(
                inner,
                ctx,
                call_id,
                call,
                Rule::InvalidArgs,
                CallStatus::InvalidArgs,
                message,
            )
            .await;
        }
    };

    let own = ctx
        .agent
        .tools()
        .get(&name)
        .copied()
        .unwrap_or(Decision::Deny);
    let (decision, reasons) = decide(&ctx.task.facts(own, name.effect(), &action));
    let requested = RunEvent::ToolCallRequested {
        call_id,
        tool: call.name.clone(),
        args: args.clone(),
        action: Some(action.clone()),
        decision: Some(decision),
        reasons: reasons.clone(),
    };
    inner.emit(ctx.at, requested).await?;

    let ended = match decision {
        Decision::Deny => {
            let rule = reasons.first().map_or("agent_policy", Rule::kind);
            failed(
                CallStatus::Refused,
                &format!("refused by rule {rule}; do not retry it"),
                None,
            )
        }
        Decision::Ask => match ask(inner, ctx, call_id, &action, &args, &reasons).await? {
            Waited::Answered(Answer::Approve { .. }) => {
                run(inner, ctx, call_id, tool, args.clone()).await?
            }
            Waited::Answered(Answer::Deny { note }) => {
                failed(CallStatus::DeniedHuman, "a person denied this call", note)
            }
            Waited::Expired => failed(
                CallStatus::Expired,
                "nobody answered the approval in time",
                None,
            ),
            Waited::Cancelled => failed(CallStatus::Cancelled, "the task was cancelled", None),
        },
        Decision::Allow => run(inner, ctx, call_id, tool, args.clone()).await?,
    };

    let refused_or_denied = matches!(
        &ended,
        Ended::Failed {
            status: CallStatus::Refused | CallStatus::DeniedHuman,
            ..
        }
    );
    finish(
        inner,
        ctx,
        call_id,
        started,
        &ended,
        refused_or_denied.then_some((name, args)),
    )
    .await?;
    Ok(render(&ended))
}

/// Not offered and invalid arguments: logged and refused before any decision. Arguments that
/// are not JSON are logged as the raw string.
async fn refuse_early(
    inner: &Inner,
    ctx: &RunCtx,
    call_id: ToolCallId,
    call: &FnCall,
    rule: Rule,
    status: CallStatus,
    message: String,
) -> Result<String, StoreError> {
    let args =
        parse_args(&call.arguments).unwrap_or_else(|_| Value::String(call.arguments.clone()));
    let requested = RunEvent::ToolCallRequested {
        call_id,
        tool: call.name.clone(),
        args,
        action: None,
        decision: None,
        reasons: vec![rule],
    };
    inner.emit(ctx.at, requested).await?;
    let ended = failed(status, &message, None);
    finish(inner, ctx, call_id, Instant::now(), &ended, None).await?;
    Ok(render(&ended))
}

async fn ask(
    inner: &Inner,
    ctx: &RunCtx,
    call_id: ToolCallId,
    action: &Action,
    args: &Value,
    reasons: &[Rule],
) -> Result<Waited, StoreError> {
    let approval_id = ApprovalId::new();
    let timeout = inner.config.approval_timeout;
    let expires_at =
        Utc::now() + chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::MAX);
    let custody = Custody {
        agent_path: vec![ctx.agent.slug().clone()],
        parent_task: ctx.task_text.clone(),
        rules: reasons.to_vec(),
        taint_sources: ctx.task.taint_sources(),
    };
    let preview = serde_json::to_string_pretty(args).unwrap_or_default();

    let rx = inner.approvals.register(approval_id);
    let requested = RunEvent::ApprovalRequested {
        approval_id,
        call_id,
        action: action.clone(),
        custody,
        preview,
        expires_at,
    };
    if let Err(error) = inner.emit(ctx.at, requested).await {
        inner.approvals.forget(approval_id);
        return Err(error);
    }

    let waited = inner
        .approvals
        .wait(approval_id, rx, timeout, &ctx.task.cancel_token())
        .await;
    let (outcome, note) = match &waited {
        Waited::Answered(Answer::Approve { note }) => (ApprovalOutcome::Approved, note.clone()),
        Waited::Answered(Answer::Deny { note }) => (ApprovalOutcome::Denied, note.clone()),
        Waited::Expired => (ApprovalOutcome::Expired, None),
        Waited::Cancelled => (ApprovalOutcome::Cancelled, None),
    };
    inner
        .emit(
            ctx.at,
            RunEvent::ApprovalResolved {
                approval_id,
                outcome,
                note,
            },
        )
        .await?;
    Ok(waited)
}

/// Runs an approved or allowed call.
async fn run(
    inner: &Inner,
    ctx: &RunCtx,
    call_id: ToolCallId,
    tool: Arc<dyn Tool>,
    args: Value,
) -> Result<Ended, StoreError> {
    inner
        .emit(ctx.at, RunEvent::ToolCallStarted { call_id })
        .await?;
    Ok(invoke(ctx, tool, args).await)
}

/// The one place a call's work starts; a tool that is not a plain `Tool::run` branches here.
async fn invoke(ctx: &RunCtx, tool: Arc<dyn Tool>, args: Value) -> Ended {
    let tool_ctx = ToolCtx {
        run_id: ctx.at.run_id,
        root_run_id: ctx.at.root_run_id,
        agent_id: ctx.agent.id(),
        trust: Trust::Trusted,
    };
    let timeout = tool.spec().timeout;
    let handle = tokio::spawn(async move { tool.run(&tool_ctx, args).await });
    let abort = handle.abort_handle();
    let cancel = ctx.task.cancel_token();

    let joined = tokio::select! {
        joined = handle => joined,
        () = sleep_or_forever(timeout) => {
            // Dropping a JoinHandle only detaches the task; abort stops it.
            abort.abort();
            return failed(CallStatus::Timeout, "the tool timed out", None);
        }
        () = cancel.cancelled() => {
            abort.abort();
            return failed(CallStatus::Cancelled, "the task was cancelled", None);
        }
    };
    match joined {
        Ok(Ok(output)) => Ended::Ok(output),
        Ok(Err(error)) => failed(CallStatus::Failed, &error.to_string(), None),
        Err(_) => failed(CallStatus::Internal, "the tool stopped unexpectedly", None),
    }
}

async fn sleep_or_forever(timeout: Option<Duration>) {
    match timeout {
        Some(timeout) => tokio::time::sleep(timeout).await,
        None => std::future::pending().await,
    }
}

/// The only terminal emit for a call. `refused` carries tool and args for the repeat check.
async fn finish(
    inner: &Inner,
    ctx: &RunCtx,
    call_id: ToolCallId,
    started: Instant,
    ended: &Ended,
    refused: Option<(ToolName, Value)>,
) -> Result<(), StoreError> {
    let (status, result_preview) = match ended {
        Ended::Ok(output) => (CallStatus::Ok, Some(preview(&output.content))),
        Ended::Failed {
            status, message, ..
        } => (*status, Some(message.clone())),
    };
    let (tool, args) = match refused {
        Some((tool, args)) => (Some(tool), Some(args)),
        None => (None, None),
    };
    let finished = RunEvent::ToolCallFinished {
        call_id,
        status,
        result_preview,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        child_run_id: None,
        tool,
        args,
    };
    inner.emit(ctx.at, finished.clone()).await?;
    ctx.task.apply(&finished);
    Ok(())
}

fn render(ended: &Ended) -> String {
    match ended {
        Ended::Ok(output) => json!({"ok": true, "result": output.content}).to_string(),
        Ended::Failed {
            status,
            message,
            note,
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

fn parse_args(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|error| format!("arguments are not valid JSON: {error}"))
}

fn preview(content: &Value) -> String {
    content.to_string().chars().take(PREVIEW_CHARS).collect()
}

fn failed(status: CallStatus, message: &str, note: Option<String>) -> Ended {
    Ended::Failed {
        status,
        message: message.to_string(),
        note,
    }
}
