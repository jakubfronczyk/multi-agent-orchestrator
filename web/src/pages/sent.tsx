import { useQuery } from "@tanstack/react-query"
import { Link, useLocation } from "react-router"
import { Inbox, Mail, MessageSquare } from "lucide-react"
import { outboxQuery } from "@/api/outbox"
import { runsQuery } from "@/api/runs"
import { toolCallsQuery } from "@/api/tool-calls"
import { AgentShape } from "@/components/agent-shape"
import { CallRow } from "@/components/call-row"
import { Page } from "@/components/page"
import { TeamTag } from "@/components/team-tag"
import { useTeamScope } from "@/lib/team-scope"
import { useInvalidateOnToolFinished } from "@/events/use-invalidate-on-tool"
import type { OutboxMessage } from "@/generated/OutboxMessage"
import type { ToolCallView } from "@/generated/ToolCallView"
import { useAgents } from "@/lib/agents"
import { sheetSearch } from "@/lib/how-this-happened"
import { dayTime } from "@/lib/time"

const SENDS = toolCallsQuery({ tool: "send_message", limit: 200 })

export function SentPage() {
  const team = useTeamScope()
  // The outbox has no `last_event_id`, so it refetches whenever the page is shown.
  const outbox = useQuery({ ...outboxQuery(team.teamId), refetchOnMount: "always", enabled: team.ready })
  const sends = useQuery({
    ...toolCallsQuery({ tool: "send_message", limit: 200, team_id: team.teamId }),
    enabled: team.ready,
  })
  useInvalidateOnToolFinished(["outbox"], "send_message")
  useInvalidateOnToolFinished(SENDS.queryKey, "send_message")

  const messages = [...(outbox.data?.messages ?? [])].sort((a, b) => b.sent_at.localeCompare(a.sent_at))
  const calls = sends.data?.tool_calls ?? []
  const stopped = calls
    .filter((call) => call.phase === "finished" && call.status !== "ok")
    .sort((a, b) => b.requested_at.localeCompare(a.requested_at))

  return (
    <Page title="Sent messages" lede="What your team sent, each time after you said yes.">
      <div className="flex items-start gap-4 rounded-lg bg-muted px-5 py-4 text-sm text-sidebar-foreground">
        <Inbox className="mt-0.5 size-[18px] flex-none" />
        <div>
          <b className="block font-medium text-foreground">Simulated outbox: nothing is delivered</b>
          When an agent sends an email or chat message, it lands here and is never delivered. Task text still goes
          to xAI, and opening a website reaches the real site.
        </div>
      </div>

      {team.pills}

      <section aria-labelledby="sent-heading">
        <h2 id="sent-heading" className="m-0 mb-3 text-sm font-medium text-sidebar-foreground">
          Sent
        </h2>
        {outbox.isPending ? null : outbox.error ? (
          <p className="m-0 text-sm text-destructive">Couldn't load the outbox. {outbox.error.message}</p>
        ) : messages.length > 0 ? (
          <div className="flex flex-col rounded-lg bg-card p-1 ring-1 ring-border">
            {messages.map((message) => (
              <SentCard key={message.id} message={message} calls={calls} teamName={team.label(message.team_id)} />
            ))}
          </div>
        ) : (
          <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
            <b className="font-medium">Nothing sent yet</b>
            <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
              When an agent sends an email or chat message after your yes, it shows up here.
            </p>
          </div>
        )}
      </section>

      {stopped.length > 0 && (
        <section aria-labelledby="stopped-heading">
          <h2 id="stopped-heading" className="m-0 mb-3 text-sm font-medium text-sidebar-foreground">
            Stopped before sending
          </h2>
          <StoppedRows calls={stopped} />
        </section>
      )}
    </Page>
  )
}

function StoppedRows({ calls }: { calls: ToolCallView[] }) {
  const runs = useQuery(runsQuery())
  return (
    <div className="flex flex-col">
      {calls.map((call) => {
        const task = runs.data?.runs.find((run) => run.id === call.root_run_id)
        return (
          <CallRow
            key={call.id}
            call={call}
            subtitle={task ? (task.title ?? task.task) : undefined}
          />
        )
      })}
    </div>
  )
}

/** The body without a leading "Subject: …" line that repeats the message's own subject. */
function bodyOf(message: OutboxMessage): string {
  const [first, ...rest] = message.body.split("\n")
  if (!message.subject || first.trim().replace(/^subject:\s*/i, "") !== message.subject.trim()) return message.body
  return rest.join("\n").replace(/^\s*\n/, "")
}

/**
 * One delivered message, collapsed to a line until opened: every send asked first (send_message
 * is never Allow), so a message in the outbox is one you said yes to. Its call opens the sheet.
 */
function SentCard({
  message,
  calls,
  teamName,
}: {
  message: OutboxMessage
  calls: ToolCallView[]
  teamName?: string
}) {
  const look = useAgents()
  const location = useLocation()
  const runs = useQuery(runsQuery())
  // The message names its own call, so two sends to one person in a run stay apart.
  const call = calls.find((c) => c.id === message.tool_call_id)
  const agent = call ? look.byId(call.agent_id) : undefined
  const task = runs.data?.runs.find((run) => run.id === message.root_run_id)
  const body = bodyOf(message)
  const Icon = message.channel === "email" ? Mail : MessageSquare
  const link = "text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"

  return (
    <details className="group border-b border-border last:border-b-0">
      <summary className="flex cursor-pointer list-none items-start gap-3 rounded-md px-3 py-3 hover:bg-accent/60 [&::-webkit-details-marker]:hidden">
        {agent ? (
          <AgentShape icon={agent.icon} colour={agent.colour} size="md" className="mt-0.5" />
        ) : (
          <Icon className="mt-0.5 size-5 flex-none text-muted-foreground" />
        )}
        <div className="flex min-w-0 flex-1 flex-col gap-0.5">
          <div className="flex min-w-0 items-baseline gap-3">
            <b className="min-w-0 flex-1 truncate font-medium">
              {message.subject ?? (message.channel === "email" ? "Email" : "Chat message")}
            </b>
            <small className="flex-none text-xs text-muted-foreground tabular-nums">{dayTime(message.sent_at)}</small>
          </div>
          <div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
            <span className="min-w-0 truncate">
              To {message.recipient}
              {agent && <>, from {agent.name}</>}
            </span>
            <TeamTag name={teamName} />
          </div>
          <p className="m-0 truncate text-sm text-muted-foreground/80 group-open:hidden">{body}</p>
        </div>
      </summary>
      <div className="flex flex-col gap-3 pr-3 pb-4 pl-11">
        <p className="m-0 max-w-[64ch] text-sm break-words whitespace-pre-wrap text-sidebar-foreground">{body}</p>
        <span className="flex flex-wrap gap-4">
          {call && (
            <Link to={{ pathname: location.pathname, search: sheetSearch(call.id, call.root_run_id) }} className={link}>
              How this happened
            </Link>
          )}
          <Link to={`/tasks/${message.root_run_id}`} className={link}>
            Open {task ? `"${(task.title ?? task.task).replace(/\.$/, "")}"` : "the task"}
          </Link>
        </span>
      </div>
    </details>
  )
}
