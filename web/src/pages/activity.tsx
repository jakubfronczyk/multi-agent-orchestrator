import { useQuery } from "@tanstack/react-query"
import { useSearchParams } from "react-router"
import { runsQuery } from "@/api/runs"
import { toolCallsQuery } from "@/api/tool-calls"
import { CallRow } from "@/components/call-row"
import { FilterPills } from "@/components/filter-pills"
import { useTeamScope } from "@/lib/team-scope"
import { Page } from "@/components/page"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { ToolCallView } from "@/generated/ToolCallView"

type Filter = "all" | "asked" | "refused" | "failed" | "outside"

const FILTERS: [Filter, string][] = [
  ["all", "Everything"],
  ["asked", "Asked you"],
  ["refused", "Not allowed"],
  ["failed", "Didn't work"],
  ["outside", "Outside content"],
]


// The same groups as the row labels (`callLabel`): a call that was not offered or had invalid
// arguments reads "Not allowed", so it filters there too.
const REFUSED = new Set(["refused", "not_offered", "invalid_args"])
const FAILED = new Set(["failed", "timeout", "internal", "child_failed"])

// The server's largest page; the list says so when it is full.
const LIMIT = 200
const CALLS = toolCallsQuery({ limit: LIMIT })

const isFilter = (value: string | null): value is Filter => FILTERS.some(([key]) => key === value)

const DAY_MS = 86_400_000
const weekday = new Intl.DateTimeFormat(undefined, { weekday: "long" })
const longDate = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" })

/** "Today", a weekday name within the last week, or the date. */
function dayLabel(iso: string): string {
  const day = new Date(iso)
  day.setHours(0, 0, 0, 0)
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  const ago = Math.round((today.getTime() - day.getTime()) / DAY_MS)
  if (ago === 0) return "Today"
  if (ago === 1) return "Yesterday"
  return ago < 7 ? weekday.format(day) : longDate.format(day)
}

export function ActivityPage() {
  const team = useTeamScope()
  const calls = useQuery({ ...toolCallsQuery({ limit: LIMIT, team_id: team.teamId }), enabled: team.ready })
  const runs = useQuery(runsQuery())
  // Rows can't be built from events alone (a request names no agent), so bursts of call events
  // become one refetch.
  useInvalidateOn(
    CALLS.queryKey,
    (event) =>
      event.type === "tool_call_requested" ||
      event.type === "tool_call_finished" ||
      event.type === "approval_requested" ||
      event.type === "tool_call_started",
    300,
  )
  const [params, setParams] = useSearchParams()
  const raw = params.get("filter")
  const filter: Filter = isFilter(raw) ? raw : "all"

  // The call itself says whether it brought outside content into its task.
  const fromOutside = (call: ToolCallView) => call.tainted
  const matches: Record<Filter, (call: ToolCallView) => boolean> = {
    all: () => true,
    asked: (call) => call.decision === "ask",
    refused: (call) => REFUSED.has(call.status ?? ""),
    failed: (call) => FAILED.has(call.status ?? ""),
    outside: fromOutside,
  }

  const all = calls.data?.tool_calls ?? []
  const shown = all.filter(matches[filter])
  const groups: [string, ToolCallView[]][] = []
  for (const call of shown) {
    const label = dayLabel(call.requested_at)
    const group = groups.at(-1)
    if (group && group[0] === label) group[1].push(call)
    else groups.push([label, [call]])
  }

  return (
    <Page
      wide
      title="Activity"
      lede={`Every tool your team used, newest first. Open any line to see who asked, what the task had read, the rule and who decided.${
        all.length === LIMIT ? ` Showing the latest ${LIMIT}.` : ""
      }`}
    >
      <FilterPills
        label="Show activity"
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
      {calls.isPending ? null : calls.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load the activity. {calls.error.message}</p>
      ) : groups.length > 0 ? (
        <div className="flex flex-col">
          {groups.map(([label, rows]) => (
            <section key={label} aria-label={label}>
              <h2 className="m-0 px-3 pt-4 pb-1 text-sm font-medium text-muted-foreground">{label}</h2>
              {rows.map((call) => {
                const task = runs.data?.runs.find((run) => run.id === call.root_run_id)
                const title = task ? (task.title ?? task.task) : undefined
                const teamName = team.label(call.team_id)
                return (
                  <CallRow
                    key={call.id}
                    call={call}
                    fromOutside={fromOutside(call)}
                    showAgent
                    subtitle={teamName && title ? `${title}, ${teamName}` : (title ?? teamName)}
                  />
                )
              })}
            </section>
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
          <b className="font-medium">{all.length === 0 ? "No activity yet" : "Nothing matches"}</b>
          <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
            {all.length === 0 ? "Every tool your team uses shows up here." : "No actions of this kind yet."}
          </p>
        </div>
      )}
    </Page>
  )
}
