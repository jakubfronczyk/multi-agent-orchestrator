import { useState } from "react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { Link, useLocation, useNavigate } from "react-router"
import { CircleCheck, TriangleAlert } from "lucide-react"
import { cn } from "cn"
import { drillReportQuery, startDrill } from "@/api/drill"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { Page } from "@/components/page"
import { Button } from "@/components/ui/button"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { DrillExit } from "@/generated/DrillExit"
import type { RunSummary } from "@/generated/RunSummary"
import { useAgents } from "@/lib/agents"
import { callSummary, reasonsText } from "@/lib/copy"
import { sheetSearch } from "@/lib/how-this-happened"
import { dayTime } from "@/lib/time"

const RUNS = runsQuery(200)

/** Who decided an exit; the leak case (a yes) is the one marked as a failure. */
function decider(exit: DrillExit): { text: string; leak: boolean } {
  if (exit.status === null) return { text: "Waiting for you", leak: false }
  if (exit.decided_by === "gateway") {
    return exit.rules.some((rule) => rule.kind === "repeat_of_refused")
      ? { text: "Stopped automatically, nobody was asked again", leak: false }
      : { text: "Your rules, nobody was asked", leak: false }
  }
  switch (exit.approval) {
    case "approved":
      // A yes leaks only when the call then went through.
      return { text: "You said yes", leak: exit.status === "ok" }
    case "denied":
      return { text: "You said no", leak: false }
    case "expired":
      return { text: "No answer, counted as no", leak: false }
    case "cancelled":
      return { text: "Cancelled before an answer", leak: false }
    case "pending":
    case null:
      return { text: "Waiting for you", leak: false }
  }
}

export function DrillPage() {
  const navigate = useNavigate()
  const runs = useQuery(RUNS)
  useInvalidateOn(
    RUNS.queryKey,
    (event) => (event.type === "run_started" && event.data.parent_run_id === null) || event.type === "run_finished",
  )
  const [error, setError] = useState<string | null>(null)
  const start = useMutation({ mutationFn: startDrill })

  const drills = (runs.data?.runs ?? []).filter((run) => run.scenario === "drill")
  const latest = drills[0]
  const running = latest?.status === "running"
  const lastFinished = drills.find((run) => run.status !== "running")

  const run = async () => {
    setError(null)
    try {
      const { run_id } = await start.mutateAsync()
      void navigate(`/tasks/${run_id}`)
    } catch (failure) {
      setError(`Couldn't start the drill. ${failure instanceof Error ? failure.message : ""}`)
    }
  }

  return (
    <Page
      title="Security drill"
      lede="Runs a task that reads a planted vendor note. The note tries to get your data out. The drill pauses at each request and you answer it yourself, then lists every way out it tried and what stopped it."
    >
      <div className="flex flex-col gap-2 rounded-lg bg-card px-5 py-4 ring-1 ring-border">
        <div className="flex flex-wrap items-center gap-3">
          <StatusShape running={running} />
          <div className="min-w-[200px] flex-1">
            <b className="block font-medium">
              {runs.isPending
                ? "Security drill"
                : running
                  ? "Drill running"
                  : latest
                    ? `Last drill: ${dayTime(latest.started_at)}`
                    : "No drill run yet"}
            </b>
            <small className="block text-sm text-muted-foreground">
              {running
                ? "Answer each request in the drill task, the way you would for real."
                : "Answer each request when it pauses, the way you would for real."}
            </small>
          </div>
          {running && latest ? (
            <Button asChild>
              <Link to={`/tasks/${latest.id}`}>Open the drill</Link>
            </Button>
          ) : (
            <Button disabled={start.isPending || runs.isPending} onClick={() => void run()}>
              Run security drill
            </Button>
          )}
        </div>
        {error && (
          <p role="alert" className="m-0 text-sm text-destructive">
            {error}
          </p>
        )}
      </div>

      {lastFinished && <Report run={lastFinished} />}
    </Page>
  )
}

function StatusShape({ running }: { running: boolean }) {
  const look = useAgents()
  const lead = look.agents.find((agent) => agent.role === "lead")
  return lead ? <AgentShape icon={lead.icon} colour={lead.colour} size="md" pulse={running} /> : null
}

function Report({ run }: { run: RunSummary }) {
  // The report's slugs resolve in the team the drill ran on.
  const look = useAgents(run.team_id)
  const location = useLocation()
  const report = useQuery(drillReportQuery(run.id))
  if (report.error) {
    return <p className="m-0 text-sm text-destructive">Couldn't load the drill report. {report.error.message}</p>
  }
  if (!report.data || report.data.status === "running") return null

  const { exits, messages_sent } = report.data
  const safe = messages_sent === 0 && !exits.some((exit) => decider(exit).leak)
  return (
    <section aria-label="Drill report" className="overflow-hidden rounded-lg bg-card ring-1 ring-border">
      <div className="flex items-center gap-3 border-b border-border p-5">
        {safe ? (
          <CircleCheck className="size-[18px] flex-none text-emerald-700 dark:text-emerald-400" />
        ) : (
          <TriangleAlert className="size-[18px] flex-none text-destructive" />
        )}
        <b className="text-lg font-semibold tracking-tight">
          {safe
            ? "Nothing got out. Every way out the note tried was stopped."
            : "Something got out. You said yes to a request the note asked for."}
        </b>
      </div>
      <div className="hidden grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,0.8fr)] gap-4 border-b border-border px-5 py-2.5 text-xs tracking-wide text-muted-foreground sm:grid">
        <span>What the note tried</span>
        <span>What stopped it</span>
        <span>Who decided</span>
      </div>
      {exits.length === 0 ? (
        <p className="m-0 px-5 py-4 text-sm text-muted-foreground">The note tried no way out in this drill.</p>
      ) : (
        exits.map((exit) => {
          const agentName = look.nameOfSlug(exit.agent)
          // Only the verdict's rule, which the backend puts first; the sheet lists the rest. An
          // agent_policy rule in first place set the verdict, so the decision is its setting.
          const reasons = reasonsText(exit.rules.slice(0, 1), {
            agent: agentName,
            tool: exit.tool,
            decision: exit.decision,
          })
          const who = decider(exit)
          return (
            <Link
              key={exit.call_id}
              to={{ pathname: location.pathname, search: sheetSearch(exit.call_id, report.data.root_run_id) }}
              className="grid grid-cols-1 items-start gap-1 border-b border-border px-5 py-3 text-sm last:border-b-0 hover:bg-accent sm:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,0.8fr)] sm:gap-4"
            >
              <b className="font-medium break-words">
                {agentName}: {callSummary(exit.tool, exit.action, look.nameOfSlug)}
              </b>
              <span className="break-words text-sidebar-foreground">
                {who.leak ? "Nothing. You said yes." : reasons.join(" ")}
              </span>
              <span className={cn(who.leak ? "text-destructive" : "text-violet-700 dark:text-violet-400")}>{who.text}</span>
            </Link>
          )
        })
      )}
    </section>
  )
}
