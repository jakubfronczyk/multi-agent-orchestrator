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
    // Offered on a send card to someone other than Anna, until a no with a note has gone out.
    cardNote: ({ card, approvals }) =>
      card?.tool === "send_message" &&
      !(card.action?.target.kind === "recipient" && card.action.target.address === ANNA_ADDRESS) &&
      !approvals.some((a) => a.status === "denied" && a.note)
        ? ANNA_NOTE
        : null,
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

/** The free scripted security drill, started from Home with its own endpoint and fixed task. */
export interface DemoStart {
  id: "drill"
  label: string
  detail: string
  /** The teammates the script hands work to; the team's Lead must target each of them. */
  slugs: string[]
}

export const DEMO_STARTS: DemoStart[] = [
  { id: "drill", label: "Security drill", detail: "A vendor email tries to get your notes out", slugs: ["reader", "messenger"] },
]
