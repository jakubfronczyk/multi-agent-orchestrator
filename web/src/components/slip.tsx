import { useRef, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useLocation } from "react-router"
import { ChevronDown, TriangleAlert } from "lucide-react"
import { cn } from "cn"
import { answerApproval } from "@/api/approvals"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { refetchNow } from "@/events/refetch"
import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import type { AnswerDecision } from "@/generated/AnswerDecision"
import type { ApprovalView } from "@/generated/ApprovalView"
import { useAgents } from "@/lib/agents"
import { isApiError } from "@/lib/api"
import {
  actionCopy,
  capitalize,
  formatLeft,
  listText,
  NOTE_PRESETS,
  resolvedText,
  sourceLabel,
  splitPreview,
} from "@/lib/copy"
import { sheetSearch } from "@/lib/how-this-happened"
import { clockTime } from "@/lib/time"
import { useNow } from "@/lib/use-now"

const TONE = {
  yes: "text-emerald-700 dark:text-emerald-400",
  no: "text-violet-700 dark:text-violet-400",
  quiet: "text-muted-foreground",
}

/**
 * One approval card, pending or resolved. The countdown is a client clock; the card turns
 * resolved only when the server says so (`status`), so a card at 0:00 waits for that event.
 */
export function Slip({ approval }: { approval: ApprovalView }) {
  const look = useAgents()
  const location = useLocation()
  const client = useQueryClient()
  const runs = useQuery(runsQuery())
  const [noteOpen, setNoteOpen] = useState(false)
  const [note, setNote] = useState("")
  const sent = useRef(false)

  const pending = approval.status === "pending"
  const now = useNow(1000, pending)
  const agentSlug = approval.custody.agent_path.at(-1)
  const agent = agentSlug ? look.bySlug(agentSlug) : undefined
  const agentName = agentSlug ? look.nameOfSlug(agentSlug) : "A teammate"
  const run = runs.data?.runs.find((r) => r.id === approval.root_run_id)
  const taskTitle = run ? (run.title ?? run.task) : null
  const copy = actionCopy(approval.tool, approval.action, look.nameOfSlug)
  const preview = splitPreview(approval.tool, approval.preview)

  const span = Date.parse(approval.expires_at) - Date.parse(approval.created_at)
  const left = Date.parse(approval.expires_at) - now
  const sources = approval.custody.taint_sources.length > 0
    ? approval.custody.taint_sources
    : approval.custody.parent_task_sources
  const offList = approval.custody.rules.flatMap((rule) => (rule.kind === "off_allowlist" ? [rule.host] : []))

  const answer = useMutation({
    mutationFn: (decision: AnswerDecision) =>
      answerApproval(approval.id, { decision, note: note.trim() || undefined }),
    onSettled: () => refetchNow(client, ["approvals"]),
    onError: (error) => {
      // 409: answered elsewhere, expired or cancelled; the refetch shows that state.
      sent.current = isApiError(error, 409)
    },
  })
  const send = (decision: AnswerDecision) => {
    if (sent.current) return
    sent.current = true
    answer.mutate(decision)
  }

  const resolved = resolvedText(approval)
  const failed = answer.isError && !isApiError(answer.error, 409)

  return (
    <article
      aria-label={capitalize(copy.ask)}
      className="relative grid animate-in grid-cols-1 overflow-hidden rounded-lg bg-card shadow-lg ring-1 ring-border duration-300 fade-in slide-in-from-bottom-1 sm:grid-cols-[120px_minmax(0,1fr)]"
    >
      <div
        className={cn(
          "relative flex flex-row flex-wrap items-center gap-x-2 gap-y-1 border-b-[1.5px] border-dashed border-input bg-muted px-4 py-3",
          "sm:flex-col sm:flex-nowrap sm:items-start sm:gap-1 sm:border-r-[1.5px] sm:border-b-0 sm:px-4 sm:py-5",
          // Ticket notches where the tear line meets the edge.
          "before:absolute before:-bottom-[9px] before:-left-[9px] before:size-4 before:rounded-full before:bg-background before:ring-1 before:ring-border before:ring-inset",
          "after:absolute after:-right-[9px] after:-bottom-[9px] after:size-4 after:rounded-full after:bg-background after:ring-1 after:ring-border after:ring-inset",
          "sm:before:-top-[9px] sm:before:-right-[9px] sm:before:bottom-auto sm:before:left-auto",
          !pending && "opacity-70",
        )}
      >
        {agent ? (
          <AgentShape
            icon={agent.icon}
            colour={pending ? undefined : agent.colour}
            size="lg"
            className={pending ? "text-amber-800 dark:text-amber-300" : undefined}
          />
        ) : null}
        <span className="text-sm font-medium text-sidebar-foreground sm:mt-2">{agentName}</span>
        <span className="hidden flex-1 sm:block sm:min-h-3" />
        {pending ? (
          <>
            <span className="ml-auto text-lg font-semibold tracking-tight tabular-nums sm:ml-0" role="timer">
              {formatLeft(left)}
            </span>
            <small className="hidden text-xs text-muted-foreground sm:block">then it counts as no</small>
            <span className="mt-1 block h-[3px] basis-full overflow-hidden sm:basis-auto rounded-full bg-border sm:mt-2 sm:w-full">
              <i
                className="block h-full rounded-full bg-amber-700 transition-[width] duration-1000 ease-linear dark:bg-amber-300"
                style={{ width: `${Math.max(0, Math.min(1, left / span)) * 100}%` }}
              />
            </span>
          </>
        ) : (
          <>
            <span className="ml-auto text-lg font-semibold tracking-tight sm:ml-0">
              {approval.status === "expired" ? "No answer" : approval.status === "cancelled" ? "Closed" : "Answered"}
            </span>
            {approval.resolved_at && (
              <small className="hidden text-xs text-muted-foreground sm:block">
                at {clockTime(approval.resolved_at)}
              </small>
            )}
          </>
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-2 p-5">
        <span className="text-sm text-muted-foreground">
          <b className="font-medium text-sidebar-foreground">{agentName}</b> wants to {copy.verb}
          {taskTitle && <>, in {taskTitle}</>}
        </span>
        <h3 className="m-0 text-base leading-snug font-semibold break-words">{capitalize(copy.ask)}?</h3>
        {sources.length > 0 && (
          <p className="m-0 flex items-start gap-2 text-sm text-amber-800 dark:text-amber-300">
            <TriangleAlert className="mt-0.5 size-4 flex-none" />
            This task read {listText(sources.map(sourceLabel))}.
          </p>
        )}
        {offList.map((host) => (
          <p key={host} className="m-0 flex items-start gap-2 text-sm text-amber-800 dark:text-amber-300">
            <TriangleAlert className="mt-0.5 size-4 flex-none" />
            {host} is not on your allowed sites.
          </p>
        ))}

        <details className="group text-sm">
          <summary className="flex w-max cursor-pointer list-none items-center gap-1 text-sidebar-foreground [&::-webkit-details-marker]:hidden">
            {copy.peek}
            <ChevronDown className="size-4 transition-transform group-open:rotate-180" />
          </summary>
          <div className="mt-2 flex max-w-[64ch] flex-col gap-1 border-l-2 border-border py-0.5 pl-4 text-sidebar-foreground">
            {preview.head.map(([label, value]) => (
              <div key={label} className="text-sm text-muted-foreground">
                {label} <span className="break-all text-sidebar-foreground">{value}</span>
              </div>
            ))}
            {preview.body && <p className="m-0 break-words whitespace-pre-wrap">{preview.body}</p>}
          </div>
        </details>

        {pending ? (
          <>
            {noteOpen && (
              <div className="mt-1 flex flex-col gap-2">
                <Textarea
                  autoFocus
                  aria-label="Note for the agent"
                  placeholder="Goes to Lead with your answer"
                  value={note}
                  onChange={(event) => setNote(event.target.value)}
                  className="min-h-16 rounded-md border-0 bg-muted text-sm shadow-none focus-visible:ring-1 dark:bg-muted"
                />
                <div className="flex flex-wrap gap-1.5">
                  {NOTE_PRESETS.map((preset) => (
                    <button
                      key={preset}
                      type="button"
                      onClick={() => setNote(preset)}
                      className="h-8 rounded-full px-3 text-sm text-sidebar-foreground ring-1 ring-border ring-inset hover:text-foreground hover:ring-input"
                    >
                      {preset}
                    </button>
                  ))}
                </div>
              </div>
            )}
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <Button disabled={answer.isPending || answer.isSuccess} onClick={() => send("approve")}>
                {copy.yes}
              </Button>
              <Button variant="outline" disabled={answer.isPending || answer.isSuccess} onClick={() => send("deny")}>
                {copy.no}
              </Button>
              {!noteOpen && (
                <Button variant="ghost" className="px-3 text-sidebar-foreground" onClick={() => setNoteOpen(true)}>
                  Add a note
                </Button>
              )}
              <Link
                to={{ pathname: location.pathname, search: sheetSearch(approval.tool_call_id, approval.root_run_id) }}
                className="ml-auto text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground hover:decoration-current"
              >
                How this happened
              </Link>
            </div>
            {failed && (
              <p role="alert" className="m-0 text-sm text-destructive">
                Couldn't send your answer. {answer.error.message}
              </p>
            )}
          </>
        ) : (
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <p className={cn("m-0 text-sm", TONE[resolved.tone])}>{resolved.label}</p>
            <Link
              to={{ pathname: location.pathname, search: sheetSearch(approval.tool_call_id, approval.root_run_id) }}
              className="ml-auto text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground hover:decoration-current"
            >
              How this happened
            </Link>
          </div>
        )}
      </div>
    </article>
  )
}
