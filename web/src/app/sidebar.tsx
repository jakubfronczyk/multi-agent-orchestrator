import { useQuery } from "@tanstack/react-query"
import { NavLink } from "react-router"
import { Clock, Folder, Inbox, List, NotebookText, Plus, Send, Target, Users, type LucideIcon } from "lucide-react"
import { cn } from "cn"
import { approvalsQuery } from "@/api/approvals"
import { agentsQuery } from "@/api/agents"
import { healthQuery } from "@/api/meta"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { useHubStatus, type HubStatus } from "@/events/event-hub"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { RunSummary } from "@/generated/RunSummary"
import { Appearance } from "./appearance"

const NAV: [string, LucideIcon, string][] = [
  ["/needs", Inbox, "Needs your OK"],
  ["/tasks", List, "Tasks"],
  ["/team", Users, "Your team"],
  ["/notes", NotebookText, "Team notes"],
  ["/files", Folder, "Files"],
  ["/sent", Send, "Sent messages"],
  ["/activity", Clock, "Activity"],
  ["/drill", Target, "Security drill"],
]

const CONNECTION: Record<HubStatus, string> = {
  connecting: "Connecting…",
  open: "Live",
  reconnecting: "Reconnecting, replaying missed events",
}

const DAY_MS = 86_400_000

const rowClass = ({ isActive }: { isActive: boolean }) =>
  cn(
    "flex h-9 w-full flex-none items-center gap-3 rounded-md px-3 text-left text-sm text-sidebar-foreground hover:bg-sidebar-accent hover:text-foreground",
    isActive && "bg-sidebar-accent font-medium text-foreground",
  )

/** Splits finished tasks into today and the six days before, by local start date. */
function historyGroups(runs: RunSummary[]) {
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  const weekStart = today.getTime() - 6 * DAY_MS
  const done = runs.filter((run) => run.status !== "running")
  return {
    today: done.filter((run) => Date.parse(run.started_at) >= today.getTime()),
    week: done.filter((run) => {
      const started = Date.parse(run.started_at)
      return started < today.getTime() && started >= weekStart
    }),
  }
}

export function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  const health = useQuery(healthQuery)
  const runs = useQuery(runsQuery())
  const pending = useQuery(approvalsQuery("pending"))
  const lead = useQuery({ ...agentsQuery, select: (agents) => agents.find((a) => a.role === "lead") })
  const status = useHubStatus()

  useInvalidateOn(runsQuery().queryKey, (event) =>
    event.type === "run_started"
      ? event.data.parent_run_id === null
      : event.type === "run_finished" && event.run_id === event.root_run_id,
  )
  useInvalidateOn(approvalsQuery("pending").queryKey, (event) =>
    event.type === "approval_requested" ||
    event.type === "approval_resolved" ||
    (event.type === "run_finished" && event.run_id === event.root_run_id),
  )

  const list = runs.data?.runs ?? []
  const running = list.filter((run) => run.status === "running")
  const { today, week } = historyGroups(list)
  const count = pending.data?.approvals.length ?? 0

  const taskRow = (run: RunSummary, live: boolean) => (
    <NavLink key={run.id} to={`/tasks/${run.id}`} onClick={onNavigate} className={(s) => cn(rowClass(s), "gap-2")}>
      {live && <AgentShape icon={lead.data?.icon ?? "compass"} colour={lead.data?.colour} pulse />}
      <span className="min-w-0 truncate">{run.title ?? run.task}</span>
    </NavLink>
  )

  const group = (label: string, rows: RunSummary[], live = false) =>
    rows.length > 0 && (
      <>
        <div className="px-3 pt-5 pb-1 text-sm font-medium text-muted-foreground">{label}</div>
        {rows.map((run) => taskRow(run, live))}
      </>
    )

  return (
    <nav aria-label="Navigation" className="flex h-full min-w-0 flex-col gap-0.5 overflow-auto bg-sidebar px-3 py-4">
      <div className="flex items-baseline gap-2 px-3 pt-1 pb-4 text-lg font-semibold tracking-tight">
        Orchestrator
        {health.data && (
          <small className="text-xs font-medium tracking-normal text-muted-foreground">
            {health.data.provider === "scripted" ? "no API key" : "practice"}
          </small>
        )}
      </div>

      <NavLink to="/" end onClick={onNavigate} className={rowClass}>
        <Plus className="size-[18px]" />
        New task
        <kbd className="ml-auto font-sans text-xs tracking-wider text-muted-foreground/70">⌘K</kbd>
      </NavLink>
      {NAV.map(([to, Icon, label]) => (
        <NavLink key={to} to={to} onClick={onNavigate} className={rowClass}>
          <Icon className="size-[18px]" />
          {label}
          {to === "/needs" && count > 0 && (
            <span className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-amber-100 px-1.5 text-xs font-semibold text-amber-800 tabular-nums dark:bg-amber-950 dark:text-amber-300">
              {count}
            </span>
          )}
        </NavLink>
      ))}

      {group("Running", running, true)}
      {group("Today", today)}
      {group("This week", week)}

      <div
        role="status"
        title="Connection to the event stream"
        className="mt-auto flex min-h-8 flex-none items-start gap-2 px-3 py-1.5 text-sm text-muted-foreground"
      >
        <i
          className={cn(
            "mt-1.5 size-[7px] flex-none rounded-full",
            status === "open" ? "bg-emerald-600 dark:bg-emerald-400" : "animate-pulse bg-amber-600 dark:bg-amber-400",
          )}
        />
        {CONNECTION[status]}
      </div>
      <Appearance />
    </nav>
  )
}
