import type { ApprovalView } from "@/generated/ApprovalView"

// Chips that drive a scripted demo, keyed by the task's scenario. Each one is something the
// person chooses to send; nothing here answers a question.

export interface DemoContext {
  /** The oldest card waiting in the task, if any. */
  card: ApprovalView | undefined
  approvals: ApprovalView[]
}

export interface DemoFollowUp {
  label: string
  text: string
  scenario: string
}

interface DemoScript {
  /** Text a chip puts into the composer while a card waits; sending it is a no with that note. */
  cardNote?: (ctx: DemoContext) => string | null
  /** Follow-ups offered once the task has finished. */
  followUps?: (ctx: DemoContext) => DemoFollowUp[]
}

const ANNA_NOTE = "The owner is Anna Weber, email her instead"
const ANNA_ADDRESS = "anna.weber@example.com"

const sentTo = (approvals: ApprovalView[], address: string) =>
  approvals.some(
    (a) =>
      a.status === "approved" &&
      a.tool === "send_message" &&
      a.action?.target.kind === "recipient" &&
      a.action.target.address === address,
  )

const DEMOS: Record<string, DemoScript> = {
  s1: {
    // Offered on a send card until a no with a note has gone out in the task.
    cardNote: ({ card, approvals }) =>
      card?.tool === "send_message" && !approvals.some((a) => a.status === "denied" && a.note) ? ANNA_NOTE : null,
    // Plays `s1-followup`; pointless once Anna has been emailed.
    followUps: ({ approvals }) =>
      sentTo(approvals, ANNA_ADDRESS)
        ? []
        : [{ label: "Email Anna instead", text: "Email Anna instead", scenario: "s1-followup" }],
  },
}

export function demoCardNote(scenario: string | null | undefined, ctx: DemoContext): string | null {
  return (scenario && DEMOS[scenario]?.cardNote?.(ctx)) || null
}

export function demoFollowUps(scenario: string | null | undefined, ctx: DemoContext): DemoFollowUp[] {
  return (scenario && DEMOS[scenario]?.followUps?.(ctx)) || []
}

/** A free scripted demo started from Home: a task with a scenario, or the security drill. */
export interface DemoStart {
  id: "s1" | "clean" | "launder" | "drill"
  label: string
  detail: string
  /** Absent for the drill, which has its own endpoint and fixed task. */
  task?: string
  scenario?: string
  /** Shown instead of an error when the server answers 409. */
  conflict?: string
  /** The teammates the script hands work to; the team's Lead must target each of them. */
  slugs: string[]
}

const DESK = ["reader", "writer", "messenger"]

export const DEMO_STARTS: DemoStart[] = [
  {
    id: "s1",
    label: "Access request",
    detail: "Asks you to save, then to send",
    task: "Check access request AR-1042 against our policy and email the decision to the owner.",
    scenario: "s1",
    slugs: DESK,
  },
  {
    id: "clean",
    label: "Clean read",
    detail: "Reads only trusted files, so nothing asks you",
    task: "List access grants over 90 days and the policy rule each falls under, and save the list under reports/.",
    scenario: "clean",
    slugs: DESK,
  },
  {
    id: "launder",
    label: "Index decisions",
    detail: "Reads a file an earlier task saved as untrusted",
    task: "List every decision in decisions/, read each one, and save an index as decisions/index.md.",
    scenario: "launder",
    // The server refuses it until the access request demo has saved its decision.
    conflict: "Run the access request demo first",
    slugs: DESK,
  },
  { id: "drill", label: "Security drill", detail: "A vendor note tries to get your notes out", slugs: ["reader", "messenger"] },
]
