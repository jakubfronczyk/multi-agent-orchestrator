import type { ApprovalView } from "@/generated/ApprovalView"
import type { PlanStep } from "@/generated/PlanStep"
import type { RunView } from "@/generated/RunView"
import type { ToolCallView } from "@/generated/ToolCallView"
import type { Turn } from "@/generated/Turn"
import type { ThreadState } from "./thread-state"

/** The top-level run (turn) a run belongs to. */
export function turnRunId(runs: RunView[], runId: string): string | undefined {
  let run = runs.find((r) => r.id === runId)
  while (run?.parent_run_id) {
    const parentId = run.parent_run_id
    run = runs.find((r) => r.id === parentId)
  }
  return run?.id
}

export interface CallNode {
  call: ToolCallView
  /** The delegated run's calls, nested under its `delegate` call. */
  children: CallNode[]
  /** The delegated run itself, for its cost and status. */
  child?: RunView
}

/** A run's calls as a tree: each `delegate` call holds the calls of the run it started. */
export function callTree(state: ThreadState, runId: string): CallNode[] {
  return state.calls
    .filter((call) => call.run_id === runId && call.tool !== "plan")
    .map((call) => {
      // A finished delegate names its child; a running one is found by the child's parent call.
      const child = state.runs.find((run) =>
        call.child_run_id ? run.id === call.child_run_id : run.parent_call_id === call.id,
      )
      return { call, child, children: child ? callTree(state, child.id) : [] }
    })
}

export function countCalls(nodes: CallNode[]): number {
  return nodes.reduce((total, node) => total + 1 + countCalls(node.children), 0)
}

/** The plan a turn last recorded, from its own `plan` calls. */
export function turnPlan(state: ThreadState, turn: Turn): PlanStep[] | null {
  const planCall = state.calls.findLast(
    (call) => call.run_id === turn.run_id && call.tool === "plan" && call.action?.target.kind === "plan",
  )
  return planCall?.action?.target.kind === "plan" ? planCall.action.target.steps : null
}

export function turnApprovals(state: ThreadState, approvals: ApprovalView[], turn: Turn): ApprovalView[] {
  return approvals
    .filter((a) => a.root_run_id === state.rootRunId && turnRunId(state.runs, a.run_id) === turn.run_id)
    .sort((a, b) => a.created_at.localeCompare(b.created_at))
}

export type Tone = "own" | "yes" | "deny" | "fail" | "wait" | "run" | "quiet"

/** The decision label on a call row, from its phase, status, decision and rules. */
export function callLabel(call: ToolCallView): { text: string; tone: Tone } {
  switch (call.phase) {
    case "requested":
      return { text: "Checking", tone: "run" }
    case "waiting":
      return { text: "Waiting for you", tone: "wait" }
    case "running":
      return { text: "Running", tone: "run" }
    case "finished":
      break
  }
  switch (call.status) {
    case "ok":
      return call.decision === "ask" ? { text: "You said yes", tone: "yes" } : { text: "Allowed", tone: "own" }
    case "refused":
      return call.rules.some((rule) => rule.kind === "repeat_of_refused")
        ? { text: "Repeat stopped", tone: "deny" }
        : { text: "Not allowed", tone: "deny" }
    case "not_offered":
    case "invalid_args":
      return { text: "Not allowed", tone: "deny" }
    case "denied_human":
      return { text: "You said no", tone: "deny" }
    case "expired":
      return { text: "No answer, counted as no", tone: "quiet" }
    case "timeout":
    case "failed":
    case "internal":
      return { text: "Didn't work", tone: "fail" }
    case "cancelled":
    case "child_cancelled":
      return { text: "Cancelled", tone: "quiet" }
    case "child_failed":
      return { text: "Didn't finish", tone: "fail" }
    case "interrupted":
      return { text: "Stopped by a restart", tone: "quiet" }
    case null:
      return { text: "Running", tone: "run" }
  }
}

export const TONE_CLASS: Record<Tone, string> = {
  own: "text-sidebar-foreground",
  yes: "text-emerald-700 dark:text-emerald-400",
  deny: "text-violet-700 dark:text-violet-400",
  fail: "text-destructive",
  wait: "font-medium text-amber-800 dark:text-amber-300",
  run: "text-sidebar-foreground",
  quiet: "text-muted-foreground",
}

/** The header status: the latest turn's, overridden while a card waits. */
export function headerStatus(state: ThreadState, pendingCards: number): { text: string; tone: Tone } {
  if (pendingCards > 0) return { text: "Needs your OK", tone: "wait" }
  const latest = state.turns.at(-1)
  const run = latest && state.runs.find((r) => r.id === latest.run_id)
  switch (run?.status) {
    case "running":
      return { text: "Working", tone: "run" }
    case "completed":
      return { text: "Done", tone: "yes" }
    case "failed":
      return { text: "Didn't finish", tone: "fail" }
    case "cancelled":
      return { text: "Cancelled", tone: "quiet" }
    case "interrupted":
      return { text: "Stopped by a restart", tone: "quiet" }
    default:
      return { text: "", tone: "quiet" }
  }
}

/** The fixed line or the Lead's summary that closes a turn; null while it runs. */
export function turnEnding(turn: Turn): string | null {
  switch (turn.outcome?.kind) {
    case undefined:
      return null
    case "completed":
      return turn.summary ?? turn.outcome.output
    case "failed":
      return `Lead stopped: ${turn.outcome.message}`
    case "cancelled":
      return "You cancelled this task."
    case "interrupted":
      return "This turn was stopped by a restart."
  }
}

/** "$0" for nothing, "$0.0123" below a cent, "$1.24" above. */
export function money(micros: number): string {
  if (micros === 0) return "$0"
  const dollars = micros / 1_000_000
  return `$${dollars.toFixed(dollars < 0.01 ? 4 : 2)}`
}
