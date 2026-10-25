import { useState } from "react"
import { useQuery } from "@tanstack/react-query"
import { Link, useLocation } from "react-router"
import { cn } from "cn"
import { approvalsQuery } from "@/api/approvals"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { Page } from "@/components/page"
import { Slip } from "@/components/slip"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import type { ApprovalView } from "@/generated/ApprovalView"
import { useAgents } from "@/lib/agents"
import { actionCopy, APPROVAL_LABEL, toolLabel } from "@/lib/copy"
import { sheetSearch } from "@/lib/how-this-happened"
import { clockTime } from "@/lib/time"

const LABEL_TONE: Record<ApprovalView["status"], string> = {
  pending: "font-medium text-amber-800 dark:text-amber-300",
  approved: "text-emerald-700 dark:text-emerald-400",
  denied: "text-violet-700 dark:text-violet-400",
  expired: "text-muted-foreground",
  cancelled: "text-muted-foreground",
}

export function NeedsPage() {
  const all = useQuery(approvalsQuery())
  useInvalidateOn(
    ["approvals"],
    (event) => event.type === "approval_requested" || event.type === "approval_resolved",
  )

  // A card that was pending while this page was open stays in the stack once resolved, so its
  // outcome shows where it was answered instead of jumping to Earlier.
  const [seen, setSeen] = useState<ReadonlySet<string>>(new Set())
  const approvals = all.data?.approvals ?? []
  const pendingIds = approvals.filter((a) => a.status === "pending").map((a) => a.id)
  if (pendingIds.some((id) => !seen.has(id))) setSeen(new Set([...seen, ...pendingIds]))

  const stack = approvals
    .filter((a) => a.status === "pending" || seen.has(a.id))
    .sort((a, b) => a.created_at.localeCompare(b.created_at))
  const earlier = approvals
    .filter((a) => a.status !== "pending" && !seen.has(a.id))
    .sort((a, b) => b.created_at.localeCompare(a.created_at))

  return (
    <Page
      title="Needs your OK"
      lede="Your team waits here for a yes or no when a setting or rule asks you: before sending, before opening a site not on your list, and before saving a file once a task has read outside content. If nobody answers before a card runs out, the answer is no."
    >
      <section aria-label="Waiting for you">
        {all.isPending ? null : all.error ? (
          <p className="m-0 text-sm text-destructive">Couldn't load the cards. {all.error.message}</p>
        ) : stack.length > 0 ? (
          <div className="flex flex-col gap-3">
            {stack.map((approval) => (
              <Slip key={approval.id} approval={approval} />
            ))}
          </div>
        ) : (
          <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
            <b className="font-medium">Nothing needs your OK right now</b>
            <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
              When a teammate wants to do something your settings or rules hold back, the request shows up
              here and in its task.
            </p>
          </div>
        )}
      </section>

      {earlier.length > 0 && (
        <section aria-labelledby="earlier">
          <h2 id="earlier" className="m-0 mb-3 text-sm font-medium text-sidebar-foreground">
            Earlier
          </h2>
          <div className="flex flex-col">
            {earlier.map((approval) => (
              <EarlierRow key={approval.id} approval={approval} />
            ))}
          </div>
        </section>
      )}
    </Page>
  )
}

function EarlierRow({ approval }: { approval: ApprovalView }) {
  const look = useAgents(approval.team_id)
  const location = useLocation()
  const runs = useQuery(runsQuery())
  const slug = approval.custody.agent_path.at(-1)
  const agent = slug ? look.bySlug(slug) : undefined
  const run = runs.data?.runs.find((r) => r.id === approval.root_run_id)
  const copy = actionCopy(approval.tool, approval.action, look.nameOfSlug)
  return (
    <Link
      to={{ pathname: location.pathname, search: sheetSearch(approval.tool_call_id, approval.root_run_id) }}
      className="grid grid-cols-[16px_minmax(0,1fr)] items-center gap-x-3 gap-y-1 rounded-md px-3 py-2.5 text-sm hover:bg-accent sm:grid-cols-[48px_16px_minmax(0,1fr)_auto]"
    >
      <span className="order-3 col-start-2 text-xs text-muted-foreground tabular-nums sm:order-none sm:col-start-auto">
        {clockTime(approval.created_at)}
      </span>
      {agent ? <AgentShape icon={agent.icon} colour={agent.colour} /> : <span />}
      <span className="min-w-0">
        {slug ? look.nameOfSlug(slug) : "A teammate"} asked to {copy.ask}
        {run && <small className="block truncate text-sm text-muted-foreground">{run.title ?? run.task}</small>}
      </span>
      <span className="order-4 col-start-2 flex flex-wrap items-center gap-2 sm:order-none sm:col-start-auto sm:justify-end">
        <span className="rounded-sm bg-muted px-2 py-0.5 text-xs whitespace-nowrap text-sidebar-foreground">
          {toolLabel(approval.tool)}
        </span>
        <span className={cn("text-sm whitespace-nowrap", LABEL_TONE[approval.status])}>
          {APPROVAL_LABEL[approval.status]}
        </span>
      </span>
    </Link>
  )
}
