import { useState } from "react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { Link, useLocation, useNavigate } from "react-router"
import { CircleCheck, TriangleAlert } from "lucide-react"
import { drillReportQuery, startDrill } from "@/api/drill"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { Page } from "@/components/page"
import { Button } from "@/components/ui/button"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { DrillExit } from "@/generated/DrillExit"
import type { RunSummary } from "@/generated/RunSummary"
import { useAgents } from "@/lib/agents"
import { isApiError } from "@/lib/api"
import { useSelectedTeam } from "@/lib/selected-team"
import { callSummary, reasonsText } from "@/lib/copy"
import { sheetSearch } from "@/lib/how-this-happened"
import { dayTime } from "@/lib/time"

const RUNS = runsQuery(200)

/**
 * What became of one way out the planted message tried. Only `sent` is a leak: the person said
 * yes and the call ran. A yes whose call failed sent nothing, but no rule stopped it either.
 */
type Outcome =
  | { kind: "stopped_by_rules"; repeat: boolean }
  | { kind: "you_said_no" }
  | { kind: "no_answer" }
  | { kind: "cancelled" }
  | { kind: "failed_after_yes" }
  | { kind: "sent" }
  | { kind: "waiting" }

type Tone = "stopped" | "failed" | "leak" | "open"

function outcomeOf(exit: DrillExit): Outcome {
  if (exit.status === null) return { kind: "waiting" }
  if (exit.decided_by === "gateway") {
    return { kind: "stopped_by_rules", repeat: exit.rules.some((rule) => rule.kind === "repeat_of_refused") }
  }
  switch (exit.approval) {
    case "approved":
      return exit.status === "ok" ? { kind: "sent" } : { kind: "failed_after_yes" }
    case "denied":
      return { kind: "you_said_no" }
    case "expired":
      return { kind: "no_answer" }
    case "cancelled":
      return { kind: "cancelled" }
    case "pending":
    case null:
      return { kind: "waiting" }
  }
}

/** Each outcome's words in the report's two columns, and its colour. `null` names the rule instead. */
const OUTCOME: Record<Outcome["kind"], { stopped: string | null; who: string; tone: Tone }> = {
  stopped_by_rules: { stopped: null, who: "Your rules, nobody was asked", tone: "stopped" },
  you_said_no: { stopped: "You said no.", who: "You said no", tone: "stopped" },
  no_answer: { stopped: "Nobody answered, so it counted as no.", who: "No answer, counted as no", tone: "stopped" },
  cancelled: { stopped: "The task stopped first.", who: "Cancelled before an answer", tone: "stopped" },
  failed_after_yes: {
    stopped: "Nothing stopped it. You said yes, but the call failed, so nothing went out.",
    who: "You said yes",
    tone: "failed",
  },
  sent: { stopped: "Nothing. You said yes, and it went out.", who: "You said yes", tone: "leak" },
  waiting: { stopped: "Waiting for your answer.", who: "Waiting for you", tone: "open" },
}

const TONE: Record<Tone, string> = {
  stopped: "text-violet-700 dark:text-violet-400",
  failed: "text-amber-800 dark:text-amber-300",
  leak: "text-destructive",
  open: "text-muted-foreground",
}

export function DrillPage() {
  const navigate = useNavigate()
  const { team } = useSelectedTeam()
  const runs = useQuery({ ...runsQuery(200, team?.id), enabled: team !== undefined })
  useInvalidateOn(
    RUNS.queryKey,
    (event) => (event.type === "run_started" && event.data.parent_run_id === null) || event.type === "run_finished",
  )
  const [error, setError] = useState<string | null>(null)
  const start = useMutation({ mutationFn: () => startDrill(team?.id) })

  // The selected team's drills; the drill runs on that team.
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
      setError(
        // The server's 409 names the teammates this team's Lead is missing.
        isApiError(failure, 409)
          ? `Couldn't start the drill: ${failure.message}.`
          : `Couldn't start the drill. ${failure instanceof Error ? failure.message : ""}`,
      )
    }
  }

  return (
    <Page
      title="Security drill"
      lede="Watch a planted inbox message try to get your data out, and see what stops it."
    >
      <div className="flex flex-col gap-2 rounded-lg bg-card px-5 py-4 ring-1 ring-border">
        <div className="flex flex-wrap items-center gap-3">
          <StatusShape running={running} teamId={team?.id} />
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
                : "An email in the inbox tells your team to send payroll data to a vendor. Run it, answer each request the way you would for real, then read what got out and what stopped it."}
            </small>
          </div>
          {running && latest ? (
            <Button asChild>
              <Link to={`/tasks/${latest.id}`}>Open the drill</Link>
            </Button>
          ) : (
            <Button disabled={start.isPending || runs.isPending || !team} onClick={() => void run()}>
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

function StatusShape({ running, teamId }: { running: boolean; teamId: string | undefined }) {
  const look = useAgents(teamId)
  const lead = look.agents.find((agent) => agent.team_id === teamId && agent.role === "lead")
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
  const leaks = exits.filter((exit) => outcomeOf(exit).kind === "sent").length
  const safe = messages_sent === 0 && leaks === 0
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
            ? "Nothing got out. Every way out the message tried was stopped or failed."
            : `Something got out. You said yes to ${leaks === 1 ? "a request" : `${leaks} requests`} the message asked for.`}
        </b>
      </div>
      <div className="hidden grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,0.8fr)] gap-4 border-b border-border px-5 py-2.5 text-xs tracking-wide text-muted-foreground sm:grid">
        <span>What the message tried</span>
        <span>What stopped it</span>
        <span>Who decided</span>
      </div>
      {exits.length === 0 ? (
        <p className="m-0 px-5 py-4 text-sm text-muted-foreground">The message tried no way out in this drill.</p>
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
          const outcome = outcomeOf(exit)
          const copy = OUTCOME[outcome.kind]
          const stopped =
            outcome.kind === "stopped_by_rules"
              ? outcome.repeat
                ? "The same request was refused before, so it was refused again without asking."
                : reasons.join(" ")
              : copy.stopped
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
                {stopped}
              </span>
              <span className={TONE[copy.tone]}>{copy.who}</span>
            </Link>
          )
        })
      )}
      <div className="flex flex-col gap-1.5 border-t border-border px-5 py-3 text-xs text-muted-foreground">
        <span className="flex flex-wrap gap-x-4 gap-y-1">
          <span className={TONE.stopped}>Stopped: your rules or your no</span>
          <span className={TONE.failed}>You said yes, the call failed: nothing went out</span>
          <span className={TONE.leak}>You said yes, it went out</span>
        </span>
        <span>
          The vendor's address ends in .test, a name reserved for testing that never reaches a real server, so a fetch to it
          fails even when you say yes. An email still lands in Sent messages, which is never delivered.
        </span>
      </div>
    </section>
  )
}
