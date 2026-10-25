//! The gateway (§7.2): every function call passes here, is decided, logged and answered.
//! Each call gets exactly one output and exactly one `tool_call_finished`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};

use crate::agents::{AgentSpec, Slug};
use crate::ids::{ApprovalId, RunId, ToolCallId};
use crate::llm::FnCall;
use crate::policy::{Decision, Rule, TaintSource, Trust, decide};
use crate::runs::{Answer, ApprovalOutcome, CallStatus, Custody, RunEvent};
use crate::store::StoreError;
use crate::tools::{Action, Target, Tool, ToolCtx, ToolError, ToolName, ToolOutput, ToolSpec};

use super::approvals::Waited;
use super::run::RunCtx;
use super::{Inner, native, prompts};

const PREVIEW_CHARS: usize = 200;
/// The longest result text the model receives; past it, the text is cut and marked.
const RESULT_CHARS: usize = 64_000;

/// How a call ended, before it is rendered for the model.
pub(super) enum Ended {
    Ok(ToolOutput),
    /// A delegated run that completed; its output goes back verbatim, wrapped when tainted.
    Child {
        agent: Slug,
        run_id: RunId,
        truncated: bool,
        output: String,
        /// The task's sources when it was tainted at the child's end.
        untrusted: Option<String>,
    },
    Failed {
        status: CallStatus,
        message: String,
        note: Option<String>,
    },
}

impl Ended {
    pub(super) fn failed(status: CallStatus, message: &str, note: Option<String>) -> Ended {
        Ended::Failed {
            status,
            message: message.to_string(),
            note,
        }
    }
}

/// What runs when a call is allowed: a server tool, or one the engine runs itself.
enum Callable {
    Tool(Arc<dyn Tool>),
    Delegate,
    Plan,
}

impl Callable {
    fn check(&self, args: &Value, agent: &AgentSpec) -> Result<Action, ToolError> {
        match self {
            Callable::Tool(tool) => tool.check(args),
            Callable::Delegate => native::check_delegate(args, agent.delegate_targets()),
            Callable::Plan => native::check_plan(args, agent.delegate_targets()),
        }
    }
}

/// A tool is offered when it is in the agent's map and something can run it.
fn resolve(inner: &Inner, agent: &AgentSpec, name: ToolName) -> Option<Callable> {
    if !agent.tools().contains_key(&name) {
        return None;
    }
    match name {
        ToolName::Delegate => Some(Callable::Delegate),
        ToolName::Plan => Some(Callable::Plan),
        ToolName::MemoryRead
        | ToolName::MemorySearch
        | ToolName::MemoryWrite
        | ToolName::FileList
        | ToolName::FileRead
        | ToolName::FileWrite
        | ToolName::InboxList
        | ToolName::InboxRead
        | ToolName::HttpFetch
        | ToolName::CsvQuery
        | ToolName::SendMessage => inner.tools.get(name).map(Callable::Tool),
    }
}

/// The specs the model is shown: every tool in the agent's map that can run.
pub(super) fn offered_specs(inner: &Inner, ctx: &RunCtx) -> Vec<ToolSpec> {
    ctx.agent
        .tools()
        .keys()
        .filter_map(|name| match resolve(inner, &ctx.agent, *name)? {
            Callable::Tool(tool) => Some(tool.spec().clone()),
            Callable::Delegate => Some(native::delegate_spec(
                &ctx.task.teammates(ctx.agent.delegate_targets()),
            )),
            Callable::Plan => Some(native::plan_spec()),
        })
        .collect()
}

/// The output the model sees for this call. The only error is storage, which ends the run.
pub(super) async fn execute(
    inner: &Arc<Inner>,
    ctx: &RunCtx,
    call: &FnCall,
) -> Result<String, StoreError> {
    let call_id = ToolCallId::new();
    let started = Instant::now();

    let offered = serde_json::from_value::<ToolName>(Value::String(call.name.clone()))
        .ok()
        .and_then(|name| resolve(inner, &ctx.agent, name).map(|callable| (name, callable)));
    let Some((name, callable)) = offered else {
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
        callable
            .check(&args, &ctx.agent)
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
    let (mut decision, mut reasons) = decide(&ctx.task.facts(own, name.effect(), &action));
    if ctx.task.is_repeat_of_refused(name, &args) {
        decision = Decision::Deny;
        reasons.insert(0, Rule::RepeatOfRefused);
    }
    let requested = RunEvent::ToolCallRequested {
        call_id,
        tool: call.name.clone(),
        args: args.clone(),
        action: Some(action.clone()),
        decision: Some(decision),
        reasons: reasons.clone(),
    };
    inner.emit(ctx.at, requested).await?;

    let mut ran = true;
    let ended = match decision {
        Decision::Deny => {
            ran = false;
            let rule = reasons.first().map_or("agent_policy", Rule::kind);
            Ended::failed(
                CallStatus::Refused,
                &format!("refused by rule {rule}; do not retry it"),
                None,
            )
        }
        Decision::Ask => match ask(inner, ctx, call_id, &action, &args, &reasons).await? {
            Waited::Answered(Answer::Approve { .. }) => {
                run(inner, ctx, call_id, callable, args.clone()).await?
            }
            Waited::Answered(Answer::Deny { note }) => {
                ran = false;
                Ended::failed(CallStatus::DeniedHuman, "a person denied this call", note)
            }
            Waited::Expired => {
                ran = false;
                Ended::failed(
                    CallStatus::Expired,
                    "nobody answered the approval in time",
                    None,
                )
            }
            Waited::Cancelled => {
                ran = false;
                Ended::failed(CallStatus::Cancelled, "the task was cancelled", None)
            }
        },
        Decision::Allow => run(inner, ctx, call_id, callable, args.clone()).await?,
    };

    // Rule 4: taint is committed before any model sees the result, whatever the outcome.
    let sources = if ran && name.taints() {
        taint_sources(&ended, &action)
    } else {
        Vec::new()
    };
    for source in &sources {
        if ctx.task.claim_taint(source) {
            inner
                .emit(
                    ctx.at,
                    RunEvent::TaskTainted {
                        source: source.clone(),
                    },
                )
                .await?;
        }
    }

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
    let untrusted = (!sources.is_empty()).then(|| prompts::source_label(&sources));
    Ok(render(&ended, untrusted.as_deref()))
}

/// What a call that ran read from outside. A result says it; a failure has no result, so the
/// target names the one source (a fetched URL, a read message). A failed list names none.
fn taint_sources(ended: &Ended, action: &Action) -> Vec<TaintSource> {
    match ended {
        Ended::Ok(output) => output.taint.clone(),
        Ended::Child { .. } => Vec::new(),
        Ended::Failed { .. } => match &action.target {
            Target::Host { url, .. } => vec![TaintSource::Web { url: url.clone() }],
            Target::Message { id } => vec![TaintSource::Inbox {
                message_id: id.clone(),
            }],
            Target::File { .. }
            | Target::Recipient { .. }
            | Target::Note { .. }
            | Target::Query { .. }
            | Target::Agent { .. }
            | Target::Plan { .. } => Vec::new(),
        },
    }
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
    let ended = Ended::failed(status, &message, None);
    finish(inner, ctx, call_id, Instant::now(), &ended, None).await?;
    Ok(render(&ended, None))
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
        agent_path: ctx.agent_path.clone(),
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
        .wait(approval_id, rx, timeout, &ctx.cancel)
        .await;
    let (outcome, note) = match &waited {
        Waited::Answered(Answer::Approve { note }) => (ApprovalOutcome::Approved, note.clone()),
        Waited::Answered(Answer::Deny { note }) => (ApprovalOutcome::Denied, note.clone()),
        Waited::Expired => (ApprovalOutcome::Expired, None),
        Waited::Cancelled => (ApprovalOutcome::Cancelled, None),
    };
    let resolved = RunEvent::ApprovalResolved {
        approval_id,
        outcome,
        note,
    };
    inner.emit(ctx.at, resolved).await?;
    Ok(waited)
}

/// Runs an approved or allowed call.
async fn run(
    inner: &Arc<Inner>,
    ctx: &RunCtx,
    call_id: ToolCallId,
    callable: Callable,
    args: Value,
) -> Result<Ended, StoreError> {
    inner
        .emit(ctx.at, RunEvent::ToolCallStarted { call_id })
        .await?;
    match callable {
        Callable::Tool(tool) => Ok(run_tool(ctx, tool, args).await),
        Callable::Delegate => native::delegate(inner, ctx, args).await,
        Callable::Plan => native::plan(inner, ctx, args).await,
    }
}

async fn run_tool(ctx: &RunCtx, tool: Arc<dyn Tool>, args: Value) -> Ended {
    let tool_ctx = ToolCtx {
        run_id: ctx.at.run_id,
        root_run_id: ctx.at.root_run_id,
        agent_id: ctx.agent.id(),
        trust: if ctx.task.is_tainted() {
            Trust::Untrusted
        } else {
            Trust::Trusted
        },
    };
    let timeout = tool.spec().timeout;
    let handle = tokio::spawn(async move { tool.run(&tool_ctx, args).await });
    let abort = handle.abort_handle();

    let joined = tokio::select! {
        joined = handle => joined,
        () = sleep_or_forever(timeout) => {
            // Dropping a JoinHandle only detaches the task; abort stops it.
            abort.abort();
            return Ended::failed(CallStatus::Timeout, "the tool timed out", None);
        }
        () = ctx.cancel.cancelled() => {
            abort.abort();
            return Ended::failed(CallStatus::Cancelled, "the task was cancelled", None);
        }
    };
    match joined {
        Ok(Ok(output)) => Ended::Ok(output),
        Ok(Err(error)) => Ended::failed(CallStatus::Failed, &error.to_string(), None),
        Err(_) => Ended::failed(CallStatus::Internal, "the tool stopped unexpectedly", None),
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
    let (status, result_preview, child_run_id) = match ended {
        Ended::Ok(output) => (CallStatus::Ok, preview(&output.content.to_string()), None),
        Ended::Child { run_id, output, .. } => (CallStatus::Ok, preview(output), Some(*run_id)),
        Ended::Failed {
            status, message, ..
        } => (*status, message.clone(), None),
    };
    let (tool, args) = match refused {
        Some((tool, args)) => (Some(tool), Some(args)),
        None => (None, None),
    };
    let finished = RunEvent::ToolCallFinished {
        call_id,
        status,
        result_preview: Some(result_preview),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        child_run_id,
        tool,
        args,
    };
    inner.emit(ctx.at, finished.clone()).await?;
    ctx.task.apply(&finished);
    Ok(())
}

/// `untrusted` names this call's sources when its result is outside content.
fn render(ended: &Ended, untrusted: Option<&str>) -> String {
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
            body.to_string()
        }
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

/// Cuts on a character boundary, so the envelope stays valid JSON.
fn capped(text: &str) -> (String, bool) {
    match text.char_indices().nth(RESULT_CHARS) {
        Some((end, _)) => (text[..end].to_string(), true),
        None => (text.to_string(), false),
    }
}

fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}
