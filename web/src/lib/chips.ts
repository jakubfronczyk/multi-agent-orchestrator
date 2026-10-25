import type { ApprovalView } from "@/generated/ApprovalView"
import type { RunDetail } from "@/generated/RunDetail"
import { callSummary } from "./copy"
import { explainApproval, type Lookups } from "./how-this-happened"
import type { ThreadState } from "./thread-state"
import { callLabel } from "./thread-view"

// Question chips are answered here, from the task's record. Nothing a chip says is ever sent
// to the Lead, which only sees summaries and would invent an answer.

export type QuestionChip = "why" | "so-far" | "who" | "plan"

export const QUESTION_TEXT: Record<QuestionChip, string> = {
  why: "Why did it ask me?",
  "so-far": "What happened so far?",
  who: "Who decided each step?",
  plan: "What is the plan?",
}

export interface ChipAnswer {
  question: string
  lines: string[]
}

/** The RunDetail shape `explainApproval` reads, rebuilt from the folded state. */
function asDetail(state: ThreadState): RunDetail {
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

export function answerChip(
  chip: QuestionChip,
  state: ThreadState,
  approvals: ApprovalView[],
  look: Lookups,
): ChipAnswer {
  const question = QUESTION_TEXT[chip]
  const calls = state.calls.filter((call) => call.tool !== "plan")
  const agentName = (id: string) => look.byId(id)?.name ?? "An agent"
  switch (chip) {
    case "why": {
      const card = approvals.find((a) => a.status === "pending") ?? approvals.at(-1)
      if (!card) return { question, lines: ["Nothing in this task has asked you yet."] }
      const explained = explainApproval(card, asDetail(state), look)
      const lines = explained.hops
        .filter((hop) => hop.kind === "rule" || hop.kind === "read")
        .flatMap((hop) => hop.lines)
      return { question, lines: [`About: ${explained.summary}.`, ...lines] }
    }
    case "so-far": {
      const own = calls.filter((c) => c.status === "ok" && c.decision === "allow").length
      const asked = calls.filter((c) => c.decision === "ask").length
      const stopped = calls.filter((c) => ["refused", "not_offered", "invalid_args"].includes(c.status ?? "")).length
      const running = calls.filter((c) => c.phase !== "finished").length
      const steps = (n: number) => `${n} ${n === 1 ? "step" : "steps"}`
      return {
        question,
        lines: [
          `${steps(own)} ran on their own, ${asked} asked you, ${stopped} ${stopped === 1 ? "was" : "were"} stopped by your rules.`,
          ...(running > 0 ? [`${steps(running)} still in progress.`] : []),
        ],
      }
    }
    case "who":
      return {
        question,
        lines:
          calls.length > 0
            ? calls.map(
                (c) => `${agentName(c.agent_id)}: ${callSummary(c.tool, c.action, look.nameOfSlug)}. ${callLabel(c).text}.`,
              )
            : ["Nothing has run yet."],
      }
    case "plan":
      return {
        question,
        lines: state.plan
          ? state.plan.map(
              (step, i) => `${i + 1}. ${step.agent === "self" ? "Lead" : look.nameOfSlug(step.agent)}: ${step.goal}`,
            )
          : ["Lead hasn't recorded a plan yet."],
      }
  }
}
