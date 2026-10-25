import type { EventEnvelope } from "@/generated/EventEnvelope"
import type { PlanStep } from "@/generated/PlanStep"
import type { RunDetail } from "@/generated/RunDetail"
import type { RunView } from "@/generated/RunView"
import type { TaintSource } from "@/generated/TaintSource"
import type { ToolCallView } from "@/generated/ToolCallView"
import type { Turn } from "@/generated/Turn"

/**
 * One task as the thread shows it: the RunDetail snapshot with later events folded in.
 * Approvals are not folded here; they come from the approvals list, which is refetched on
 * approval events, so no data is both folded and refetched.
 */
export interface ThreadState {
  rootRunId: string
  runs: RunView[]
  turns: Turn[]
  calls: ToolCallView[]
  plan: PlanStep[] | null
  taintSources: TaintSource[]
  costMicros: number
}

export function initThread(detail: RunDetail): ThreadState {
  return {
    rootRunId: detail.root_run_id,
    runs: detail.runs,
    turns: detail.turns,
    calls: detail.tool_calls,
    plan: detail.plan,
    taintSources: detail.taint_sources,
    costMicros: detail.cost_micros,
  }
}

const sameSource = (a: TaintSource, b: TaintSource) => JSON.stringify(a) === JSON.stringify(b)

const updateCall = (state: ThreadState, id: string, patch: (call: ToolCallView) => ToolCallView): ThreadState => ({
  ...state,
  calls: state.calls.map((call) => (call.id === id ? patch(call) : call)),
})

const updateRun = (state: ThreadState, id: string, patch: (run: RunView) => RunView): ThreadState => ({
  ...state,
  runs: state.runs.map((run) => (run.id === id ? patch(run) : run)),
})

/**
 * Applies one event to the thread. Pure; events of other tasks, events already in the
 * snapshot (a run or call it already has) and event types the thread does not show are
 * returned unchanged, so a replay never doubles a row.
 */
export function reduceThread(state: ThreadState, event: EventEnvelope): ThreadState {
  if (event.root_run_id !== state.rootRunId) return state
  switch (event.type) {
    case "run_started": {
      if (state.runs.some((run) => run.id === event.run_id)) return state
      const run: RunView = {
        id: event.run_id,
        parent_run_id: event.data.parent_run_id,
        parent_call_id: event.data.parent_call_id,
        agent_id: event.data.agent_id,
        agent_version: event.data.agent_version,
        title: event.data.title,
        task: event.data.task,
        task_sources: event.data.task_sources,
        draft_only: event.data.draft_only,
        status: "running",
        outcome: null,
        started_at: event.ts,
        finished_at: null,
        cost_micros: 0,
      }
      // A run without a parent is a new turn, as `sqlite/runs.rs` `turns` builds them.
      const turns =
        event.data.parent_run_id === null
          ? [
              ...state.turns,
              { run_id: run.id, text: run.task, summary: null, outcome: null, draft_only: run.draft_only },
            ]
          : state.turns
      return { ...state, runs: [...state.runs, run], turns }
    }
    case "run_finished": {
      const outcome = event.data.outcome
      const status = outcome.kind
      const next = updateRun(state, event.run_id, (run) => ({ ...run, status, outcome, finished_at: event.ts }))
      return {
        ...next,
        turns: next.turns.map((turn) =>
          turn.run_id === event.run_id
            ? { ...turn, outcome, summary: outcome.kind === "completed" ? outcome.output : null }
            : turn,
        ),
      }
    }
    case "plan_recorded":
      return { ...state, plan: event.data.steps }
    case "llm_responded": {
      const cost = event.data.cost
      const next = updateRun(state, event.run_id, (run) => ({ ...run, cost_micros: run.cost_micros + cost }))
      return { ...next, costMicros: next.costMicros + cost }
    }
    case "tool_call_requested": {
      if (state.calls.some((call) => call.id === event.data.call_id)) return state
      const run = state.runs.find((r) => r.id === event.run_id)
      if (!run) return state
      const call: ToolCallView = {
        id: event.data.call_id,
        run_id: event.run_id,
        root_run_id: event.root_run_id,
        agent_id: run.agent_id,
        tool: event.data.tool,
        args: event.data.args,
        action: event.data.action,
        decision: event.data.decision,
        rules: event.data.reasons,
        phase: "requested",
        status: null,
        result_preview: null,
        child_run_id: null,
        duration_ms: null,
        requested_at: event.ts,
        finished_at: null,
      }
      return { ...state, calls: [...state.calls, call] }
    }
    case "approval_requested":
      return updateCall(state, event.data.call_id, (call) => ({ ...call, phase: "waiting" }))
    case "tool_call_started":
      return updateCall(state, event.data.call_id, (call) => ({ ...call, phase: "running" }))
    case "tool_call_finished": {
      const data = event.data
      return updateCall(state, data.call_id, (call) => ({
        ...call,
        phase: "finished",
        status: data.status,
        result_preview: data.result_preview,
        duration_ms: data.duration_ms,
        child_run_id: data.child_run_id,
        finished_at: event.ts,
      }))
    }
    case "task_tainted":
      return state.taintSources.some((source) => sameSource(source, event.data.source))
        ? state
        : { ...state, taintSources: [...state.taintSources, event.data.source] }
    default:
      return state
  }
}

/** The RunDetail shape `explainApproval` reads, rebuilt from the folded state. */
export function asDetail(state: ThreadState): RunDetail {
  return {
    root_run_id: state.rootRunId,
    runs: state.runs,
    turns: state.turns,
    tool_calls: state.calls,
    pending_approvals: [],
    plan: state.plan,
    taint_sources: state.taintSources,
    cost_micros: state.costMicros,
    last_event_id: null,
  }
}
