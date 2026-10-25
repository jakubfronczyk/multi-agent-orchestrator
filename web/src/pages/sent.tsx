import { useQuery } from "@tanstack/react-query"
import { Link, useLocation } from "react-router"
import { Inbox } from "lucide-react"
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
          <div className="flex flex-col gap-3">
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

/**
 * One delivered message. Every send asked first (send_message is never Allow), so a message
 * in the outbox is one you said yes to. Its call, named on the message, opens the sheet.
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
  const link = "text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"

  return (
    <article className="flex flex-col gap-2 rounded-lg bg-card px-5 py-4 ring-1 ring-border">
      <div className="grid grid-cols-[20px_minmax(0,1fr)] items-start gap-x-3 gap-y-1 sm:grid-cols-[20px_minmax(0,1fr)_auto]">
        {agent ? <AgentShape icon={agent.icon} colour={agent.colour} size="md" className="mt-0.5" /> : <span />}
        <div className="min-w-0">
          <b className="block font-medium break-all">
            {message.channel === "email" ? "Email" : "Chat message"} to {message.recipient} <TeamTag name={teamName} />
          </b>
          <small className="block text-sm text-muted-foreground">
            {dayTime(message.sent_at)}. You said yes.
            {task && <> In {(task.title ?? task.task).replace(/\.$/, "")}.</>}
          </small>
        </div>
        <span className="col-start-2 flex flex-wrap gap-3 sm:col-start-3 sm:justify-end">
          {call && (
            <Link to={{ pathname: location.pathname, search: sheetSearch(call.id, call.root_run_id) }} className={link}>
              How this happened
            </Link>
          )}
          <Link to={`/tasks/${message.root_run_id}`} className={link}>
            Open the task
          </Link>
        </span>
      </div>
      <div className="flex max-w-[64ch] flex-col gap-1 border-l-2 border-border py-0.5 pl-4 text-sm text-sidebar-foreground">
        <div className="text-muted-foreground">
          To <span className="break-all text-sidebar-foreground">{message.recipient}</span>
        </div>
        {message.subject && (
          <div className="text-muted-foreground">
            Subject <span className="text-sidebar-foreground">{message.subject}</span>
          </div>
        )}
        <p className="m-0 break-words whitespace-pre-wrap">{message.body}</p>
      </div>
    </article>
  )
}
