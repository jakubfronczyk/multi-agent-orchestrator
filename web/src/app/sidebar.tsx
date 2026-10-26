import { useQuery } from "@tanstack/react-query"
import { NavLink } from "react-router"
import { CircleHelp, Clock, Folder, Inbox, List, NotebookText, Plus, Send, Users, type LucideIcon } from "lucide-react"
import { cn } from "cn"
import { approvalsQuery } from "@/api/approvals"
import { agentsQuery } from "@/api/agents"
import { healthQuery } from "@/api/meta"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { TeamSwitcher } from "@/components/team-switcher"
import { Button } from "@/components/ui/button"
import { isChildRun, useHubStatus, type HubStatus } from "@/events/event-hub"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { EventEnvelope } from "@/generated/EventEnvelope"
import type { RunSummary } from "@/generated/RunSummary"
import { isMac } from "@/lib/platform"
import { useTeamNames } from "@/lib/selected-team"
import { setWelcomeOpen } from "@/lib/welcome"
import { Appearance } from "./appearance"

const NAV: [string, LucideIcon, string][] = [
  ["/needs", Inbox, "Needs your OK"],
  ["/tasks", List, "Tasks"],
  ["/team", Users, "Your team"],
  ["/notes", NotebookText, "Team notes"],
  ["/files", Folder, "Files"],
  ["/sent", Send, "Sent messages"],
  ["/activity", Clock, "Activity"],
]

const CONNECTION: Record<Exclude<HubStatus, "open">, string> = {
  connecting: "Connecting…",
  reconnecting: "Reconnecting…",
  offline: "Can't reach the server, retrying",
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

/** The favicon's mark, drawn in the current text colour so it follows the app theme. */
function Logo() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className="size-6 flex-none">
      <circle
        cx="12"
        cy="12"
        r="9"
        fill="none"
        stroke="currentColor"
        strokeWidth="3"
        pathLength="60"
        strokeDasharray="16.5 3.5"
        transform="rotate(-78 12 12)"
      />
      <path fill="currentColor" d="M12 7.6 16.4 12 12 16.4 7.6 12z" />
    </svg>
  )
}

export function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  const health = useQuery(healthQuery)
  const runs = useQuery(runsQuery())
  const pending = useQuery(approvalsQuery("pending"))
  const agents = useQuery(agentsQuery)
  const status = useHubStatus()
  const names = useTeamNames()

  // A follow-up turn ends with its own run id, not the root's, so only known delegated runs are
  // skipped; a run whose start this app never saw refetches.
  const turnFinished = (event: EventEnvelope) => event.type === "run_finished" && !isChildRun(event.run_id)
  useInvalidateOn(runsQuery().queryKey, (event) =>
    event.type === "run_started" ? event.data.parent_run_id === null : turnFinished(event),
  )
  useInvalidateOn(
    approvalsQuery("pending").queryKey,
    (event) => event.type === "approval_requested" || event.type === "approval_resolved" || turnFinished(event),
  )

  const list = runs.data?.runs ?? []
  const running = list.filter((run) => run.status === "running")
  const { today, week } = historyGroups(list)
  const count = pending.data?.approvals.length ?? 0

  const taskRow = (run: RunSummary, live: boolean) => {
    const agent = agents.data?.find((a) => a.id === run.agent_id)
    return (
      <NavLink key={run.id} to={`/tasks/${run.id}`} onClick={onNavigate} className={(s) => cn(rowClass(s), "gap-2")}>
        {live && agent && <AgentShape icon={agent.icon} colour={agent.colour} pulse />}
        <span className="min-w-0 flex-1 truncate">{run.title ?? run.task}</span>
        {/* Every team's tasks are listed; with more than one team each row names its own. */}
        {names.many && (
          <small className="max-w-[40%] flex-none truncate text-xs font-normal text-muted-foreground">
            {names.nameOf(run.team_id)}
          </small>
        )}
      </NavLink>
    )
  }

  const group = (label: string, rows: RunSummary[], live = false) =>
    rows.length > 0 && (
      <>
        <div className="px-3 pt-5 pb-1 text-sm font-medium text-muted-foreground">{label}</div>
        {rows.map((run) => taskRow(run, live))}
      </>
    )

  return (
    <nav aria-label="Navigation" className="flex h-full min-w-0 flex-col gap-0.5 overflow-auto bg-sidebar px-3 py-4">
      <div className="flex items-center gap-2.5 px-3 pt-1 pb-4 text-lg font-semibold tracking-tight">
        <Logo />
        Orchestrator
        {health.data?.provider === "scripted" && (
          <small className="text-xs font-medium tracking-normal text-muted-foreground">no API key</small>
        )}
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="What is this app?"
          title="What is this app?"
          className="ml-auto text-muted-foreground"
          onClick={() => {
            onNavigate?.()
            setWelcomeOpen(true)
          }}
        >
          <CircleHelp className="size-[18px]" />
        </Button>
      </div>

      <TeamSwitcher onNavigate={onNavigate} />

      <NavLink to="/" end onClick={onNavigate} className={rowClass}>
        <Plus className="size-[18px]" />
        New task
        <kbd className="ml-auto font-sans text-xs tracking-wider text-muted-foreground/70">
          {isMac ? "⌘K" : "Ctrl K"}
        </kbd>
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

      {/* Task history scrolls on its own so the nav links and connection status stay in view. */}
      <div className="-mx-3 flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-3 scrollbar-thin">
        {group("Running", running, true)}
        {group("Today", today)}
        {group("This week", week)}
      </div>

      {/* Shown only while the event stream is down; a live connection needs no notice. */}
      {status !== "open" && (
        <div
          role="status"
          title="Connection to the event stream"
          className="flex min-h-8 flex-none items-start gap-2 px-3 py-1.5 text-sm text-muted-foreground"
        >
          <i className="mt-1.5 size-[7px] flex-none animate-pulse rounded-full bg-amber-600 dark:bg-amber-400" />
          {CONNECTION[status]}
        </div>
      )}
      <Appearance />
    </nav>
  )
}
