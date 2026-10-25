import { Link } from "react-router"
import { cn } from "cn"
import { AgentShape } from "@/components/agent-shape"
import type { RunSummary } from "@/generated/RunSummary"
import { useAgents } from "@/lib/agents"
import { clockTime, dayTime } from "@/lib/time"
import { money, TONE_CLASS, type Tone } from "@/lib/thread-view"

function rowStatus(run: RunSummary): { text: string; tone: Tone } {
  if (run.pending_approvals > 0) return { text: "Needs your OK", tone: "wait" }
  switch (run.status) {
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
  }
}

/** The outcome line under the title; none while a card waits, since the status says so. */
function outcomeLine(run: RunSummary): string | null {
  if (run.pending_approvals > 0) return null
  switch (run.outcome?.kind) {
    case "completed":
      return run.outcome.output.split("\n")[0]
    case "failed":
      return run.outcome.message
    case "cancelled":
      return "Cancelled"
    case "interrupted":
      return "Stopped by a restart"
    case undefined:
      return "Working"
  }
}

function subtitle(run: RunSummary): string | null {
  const parts = [outcomeLine(run), run.refusals > 0 ? `${run.refusals} stopped by your rules` : null]
  return parts.filter((part) => part !== null).join(", ") || null
}

const isToday = (iso: string) => new Date(iso).toDateString() === new Date().toDateString()

/** One task as a row that opens its thread: agents, title, outcome line, status, time, cost. */
export function TaskRow({ run }: { run: RunSummary }) {
  const look = useAgents()
  const status = rowStatus(run)
  const running = run.status === "running"
  return (
    <Link
      to={`/tasks/${run.id}`}
      className="grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-0.5 rounded-md p-3 text-left hover:bg-accent sm:grid-cols-[44px_minmax(0,1fr)_auto]"
    >
      <span className="row-span-2 hidden items-center gap-1 sm:flex">
        {run.agents.slice(0, 3).map((id) => {
          // An agent deleted since the task ran has no record: it gets the plain hexagon.
          const agent = look.byId(id)
          return <AgentShape key={id} icon={agent?.icon ?? ""} colour={agent?.colour} pulse={running} />
        })}
      </span>
      <span className="row-span-2 min-w-0">
        <b className="block truncate text-base font-medium">{run.title ?? run.task}</b>
        {subtitle(run) && <small className="block truncate text-sm text-muted-foreground">{subtitle(run)}</small>}
      </span>
      <span className={cn("justify-self-end text-sm whitespace-nowrap", TONE_CLASS[status.tone])}>{status.text}</span>
      <span className="justify-self-end text-xs whitespace-nowrap text-muted-foreground tabular-nums">
        {isToday(run.started_at) ? clockTime(run.started_at) : dayTime(run.started_at)},{" "}
        {/* A scripted task costs nothing until a free-text follow-up goes to the live model. */}
        {run.cost_micros === 0 && run.scenario ? "free demo" : money(run.cost_micros)}
      </span>
    </Link>
  )
}
