import { Link, useLocation } from "react-router"
import { cn } from "cn"
import { AgentShape } from "@/components/agent-shape"
import type { ToolCallView } from "@/generated/ToolCallView"
import { useAgents } from "@/lib/agents"
import { callSummary, toolLabel } from "@/lib/copy"
import { sheetSearch } from "@/lib/how-this-happened"
import { clockTime } from "@/lib/time"
import { callLabel, money, TONE_CLASS, type CallNode } from "@/lib/thread-view"

/**
 * One call as a row that opens its "How this happened" sheet. "From outside" appears when the
 * call itself brought outside content into its task, as the server records it.
 */
export function CallRow({
  call,
  childCost,
  subtitle,
  fromOutside,
  showAgent,
}: {
  call: ToolCallView
  childCost?: number
  subtitle?: string
  /** Overrides the call's own flag, for a list that marks outside content its own way. */
  fromOutside?: boolean
  /** Names the agent before the call, for rows not nested under a hand-off. */
  showAgent?: boolean
}) {
  const look = useAgents(call.team_id)
  const location = useLocation()
  const agent = look.byId(call.agent_id)
  const label = callLabel(call)
  const outside = fromOutside ?? call.tainted
  return (
    <Link
      to={{ pathname: location.pathname, search: sheetSearch(call.id, call.root_run_id) }}
      aria-label={`${agent?.name ?? "Agent"}: ${callSummary(call.tool, call.action, look.nameOfSlug)}, ${label.text}`}
      className="grid w-full grid-cols-[16px_minmax(0,1fr)] items-center gap-x-3 gap-y-1 rounded-md px-3 py-2.5 text-left text-sm hover:bg-accent sm:grid-cols-[48px_16px_minmax(0,1fr)_auto]"
    >
      <span className="order-3 col-start-2 text-xs text-muted-foreground tabular-nums sm:order-none sm:col-start-auto">
        {clockTime(call.requested_at)}
      </span>
      {agent ? (
        <AgentShape
          icon={agent.icon}
          colour={call.phase === "waiting" ? undefined : agent.colour}
          pulse={call.phase === "running" || call.phase === "requested"}
          className={call.phase === "waiting" ? "text-amber-800 dark:text-amber-300" : undefined}
        />
      ) : (
        <span />
      )}
      <span className="min-w-0 break-words">
        {showAgent && <span className="font-medium">{agent?.name ?? "An agent"}: </span>}
        {callSummary(call.tool, call.action, look.nameOfSlug)}
        {subtitle && <small className="block truncate text-sm text-muted-foreground">{subtitle}</small>}
      </span>
      <span className="order-4 col-start-2 flex flex-wrap items-center gap-2 sm:order-none sm:col-start-auto sm:justify-end">
        <span className="rounded-sm bg-muted px-2 py-0.5 text-xs whitespace-nowrap text-sidebar-foreground">
          {toolLabel(call.tool)}
        </span>
        {outside && (
          <span className="rounded-full px-2 text-xs whitespace-nowrap text-amber-800 ring-1 ring-current ring-inset dark:text-amber-300">
            From outside
          </span>
        )}
        {childCost !== undefined && childCost > 0 && (
          <span className="text-xs whitespace-nowrap text-muted-foreground tabular-nums">{money(childCost)}</span>
        )}
        <span className={cn("text-sm whitespace-nowrap", TONE_CLASS[label.tone])}>{label.text}</span>
      </span>
    </Link>
  )
}

/** A run's calls with each delegated run's calls nested under its `delegate` row. */
export function CallTree({ nodes }: { nodes: CallNode[] }) {
  return (
    <div className="flex flex-col">
      {nodes.map((node) => (
        <div key={node.call.id}>
          <CallRow call={node.call} childCost={node.child?.cost_micros} />
          {node.children.length > 0 && (
            <div className="ml-[26px] border-l border-border pl-2">
              <CallTree nodes={node.children} />
            </div>
          )}
        </div>
      ))}
    </div>
  )
}
