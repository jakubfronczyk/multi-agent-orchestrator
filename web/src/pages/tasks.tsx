import { useQuery } from "@tanstack/react-query"
import { useSearchParams } from "react-router"
import { runsQuery } from "@/api/runs"
import { FilterPills } from "@/components/filter-pills"
import { Page } from "@/components/page"
import { TaskRow } from "@/components/task-row"
import { useTeamScope } from "@/lib/team-scope"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { RunSummary } from "@/generated/RunSummary"
import { money } from "@/lib/thread-view"

type Filter = "all" | "needs" | "working" | "done" | "unfinished"

const FILTERS: [Filter, string][] = [
  ["all", "All"],
  ["needs", "Needs your OK"],
  ["working", "Working"],
  ["done", "Done"],
  ["unfinished", "Didn't finish"],
]

/** The one filter a task falls under besides All; a new RunStatus fails the build here. */
function category(run: RunSummary): Exclude<Filter, "all"> {
  if (run.pending_approvals > 0) return "needs"
  switch (run.status) {
    case "running":
      return "working"
    case "completed":
      return "done"
    case "failed":
    case "cancelled":
    case "interrupted":
      return "unfinished"
    default: {
      const unhandled: never = run.status
      throw new Error(`Unhandled run status: ${String(unhandled)}`)
    }
  }
}

// The server's largest page; the list says so when it is full.
const LIMIT = 200

const isFilter = (value: string | null): value is Filter => FILTERS.some(([key]) => key === value)

export function TasksPage() {
  const runs = useQuery(runsQuery(LIMIT))
  // A new task, a turn ending, and a card opening or closing all change rows here.
  useInvalidateOn(
    runsQuery(LIMIT).queryKey,
    (event) =>
      (event.type === "run_started" && event.data.parent_run_id === null) ||
      event.type === "run_finished" ||
      event.type === "approval_requested" ||
      event.type === "approval_resolved",
  )
  const [params, setParams] = useSearchParams()
  const raw = params.get("filter")
  const filter: Filter = isFilter(raw) ? raw : "all"
  const team = useTeamScope()

  const all = (runs.data?.runs ?? []).filter((run) => team.shows(run.team_id))
  const shown = filter === "all" ? all : all.filter((run) => category(run) === filter)
  const spend = shown.reduce((total, run) => total + run.cost_micros, 0)

  return (
    <Page
      title="Tasks"
      lede={`Every task you've given, and how it ended. Open one to see the plan, what each teammate did, and to give its Lead a follow-up.${
        runs.data
          ? ` Model spend for the ${shown.length} ${shown.length === 1 ? "task" : "tasks"} shown: ${money(spend)}.${
              all.length === LIMIT ? ` Showing the latest ${LIMIT}.` : ""
            }`
          : ""
      } Scripted demos are free.`}
    >
      <FilterPills
        label="Show tasks"
        options={FILTERS}
        value={filter}
        onChange={(next) =>
          setParams(
            (current) => {
              const updated = new URLSearchParams(current)
              if (next === "all") updated.delete("filter")
              else updated.set("filter", next)
              return updated
            },
            { replace: true },
          )
        }
      />
      {team.pills}
      {runs.isPending ? null : runs.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load the tasks. {runs.error.message}</p>
      ) : shown.length > 0 ? (
        <div className="flex flex-col">
          {shown.map((run) => (
            <TaskRow key={run.id} run={run} teamName={team.label(run.team_id)} />
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
          <b className="font-medium">{all.length === 0 ? "No tasks yet" : "No tasks here"}</b>
          <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
            {all.length === 0 ? "Start one from New task." : "Try another filter."}
          </p>
        </div>
      )}
    </Page>
  )
}
