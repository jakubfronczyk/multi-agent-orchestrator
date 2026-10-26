import { useLayoutEffect, useRef, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useParams } from "react-router"
import { ChevronDown, CircleCheck, ScrollText, TriangleAlert } from "lucide-react"
import { cn } from "cn"
import { answerApproval, approvalsQuery } from "@/api/approvals"
import { cancelRun, postFollowUp, runQuery, runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { CallTree } from "@/components/call-row"
import { Composer, type SubmitOptions } from "@/components/composer"
import { Slip } from "@/components/slip"
import { Button } from "@/components/ui/button"
import { refetchNow } from "@/events/refetch"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import { useLiveState } from "@/events/use-live-state"
import type { ApprovalView } from "@/generated/ApprovalView"
import type { TaintSource } from "@/generated/TaintSource"
import type { Turn } from "@/generated/Turn"
import { useAgents } from "@/lib/agents"
import { isApiError } from "@/lib/api"
import { answerChip, QUESTION_TEXT, type ChipAnswer, type QuestionChip } from "@/lib/chips"
import { actionCopy, capitalize, TAINT_EFFECT, withAttachments } from "@/lib/copy"
import { demoCardNote, demoFollowUps } from "@/lib/demo"
import { initThread, reduceThread, type ThreadState } from "@/lib/thread-state"
import {
  callTree,
  countCalls,
  recordedOutcome,
  headerStatus,
  money,
  TONE_CLASS,
  turnApprovals,
  turnEnding,
  turnPlan,
} from "@/lib/thread-view"
import { useDocumentTitle } from "@/lib/use-document-title"
import { useTeamNames } from "@/lib/selected-team"
import { TeamTag } from "@/components/team-tag"
import { useNow } from "@/lib/use-now"

type Chip =
  | { kind: "question"; id: QuestionChip }
  | { kind: "fill"; text: string }
  | { kind: "follow-up"; label: string; text: string; scenario?: string }

/** What a text is written for: a no on one specific card, or a follow-up turn. */
interface NoteTarget {
  kind: "note"
  cardId: string
  title: string
}
interface FollowUpTarget {
  kind: "follow-up"
}
type Target = NoteTarget | FollowUpTarget

const sameTarget = (a: Target, b: Target) =>
  a.kind === "note" ? b.kind === "note" && a.cardId === b.cardId : b.kind === "follow-up"

const chipLabel = (chip: Chip) =>
  chip.kind === "question" ? QUESTION_TEXT[chip.id] : chip.kind === "fill" ? chip.text : chip.label

export function TaskThreadPage() {
  const { rootId = "" } = useParams()
  const detail = useQuery(runQuery(rootId))
  const state = useLiveState(detail.data, initThread, reduceThread)
  const approvalsList = useQuery(approvalsQuery())
  useInvalidateOn(["approvals"], (event) => event.type === "approval_requested" || event.type === "approval_resolved")

  if (isApiError(detail.error, 404)) {
    return <Missing text="This task doesn't exist. It may be from before the database was reset." />
  }
  if (detail.error) return <Missing text={`Couldn't load this task. ${detail.error.message}`} />
  if (!state) return null
  return (
    <Thread
      key={rootId}
      state={state}
      approvals={(approvalsList.data?.approvals ?? []).filter((a) => a.root_run_id === rootId)}
    />
  )
}

function Missing({ text }: { text: string }) {
  return (
    <div className="mx-auto w-full max-w-[720px] px-4 pt-10 md:px-6">
      <p className="m-0 text-sm text-muted-foreground">{text}</p>
      <Link to="/tasks" className="mt-2 inline-block text-sm text-sidebar-foreground underline underline-offset-[3px]">
        All tasks
      </Link>
    </div>
  )
}

function Thread({ state, approvals }: { state: ThreadState; approvals: ApprovalView[] }) {
  const look = useAgents(state.teamId)
  const client = useQueryClient()
  const runs = useQuery(runsQuery())
  const summary = runs.data?.runs.find((run) => run.id === state.rootRunId)
  const root = state.runs.find((run) => run.id === state.rootRunId)
  const rootAgent = root ? look.byId(root.agent_id) : undefined
  const rootIsLead = rootAgent?.role === "lead"
  // The task's own team, never the selection; a deleted team's task takes no follow-up.
  const names = useTeamNames()
  const teamName = names.nameOf(state.teamId)
  const teamGone = names.isDeleted(state.teamId)
  // The Lead can be renamed; a direct task's agent is named by its own name.
  const leadName = rootAgent?.name ?? "Lead"
  const latest = state.turns.at(-1)
  const latestRun = latest && state.runs.find((run) => run.id === latest.run_id)
  const running = latestRun?.status === "running"
  const pending = approvals.filter((a) => a.status === "pending")
  const card = pending[0]
  const status = headerStatus(state, pending.length)

  const [text, setText] = useState("")
  // What the text was typed as; a send in the other mode needs an explicit yes.
  const [intent, setIntent] = useState<Target | null>(null)
  const [swap, setSwap] = useState<{ message: string; options: SubmitOptions; from: Target; to: Target } | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [used, setUsed] = useState<ReadonlySet<string>>(new Set())
  const [answers, setAnswers] = useState<ChipAnswer[]>([])

  const deny = useMutation({
    mutationFn: ({ id, note }: { id: string; note: string }) => answerApproval(id, { decision: "deny", note }),
    onSettled: () => refetchNow(client, ["approvals"]),
  })
  const followUp = useMutation({
    mutationFn: (body: { text: string; scenario?: string; draft_only?: boolean }) => postFollowUp(state.rootRunId, body),
  })
  const cancel = useMutation({
    mutationFn: () => cancelRun(state.rootRunId),
    // 409: it finished meanwhile; its run_finished event updates the thread either way.
    onError: (error) => {
      if (!isApiError(error, 409)) setNotice(`Couldn't stop the task. ${error.message}`)
    },
  })
  const stop = () => {
    if (!cancel.isPending) cancel.mutate()
  }

  /** `busy` is the text for the "still running" 409: a typed follow-up keeps its text, a chip has none. */
  const sendFollowUp = async (
    body: { text: string; scenario?: string; draft_only?: boolean },
    busy = "The task is still running. Your text is kept; send it once it finishes.",
  ) => {
    try {
      await followUp.mutateAsync(body)
      return true
    } catch (error) {
      setNotice(
        !isApiError(error, 409)
          ? `Couldn't send the follow-up. ${error instanceof Error ? error.message : ""}`
          : error.message.includes("team was deleted")
            ? "This task's team was deleted, so it takes no follow-up."
            : error.message.includes("still running")
              ? busy
              : `Couldn't send the follow-up: ${error.message}.`,
      )
      return false
    }
  }

  const send = async (message: string, options: SubmitOptions, to: Target): Promise<boolean> => {
    setNotice(null)
    setSwap(null)
    if (to.kind === "note") {
      try {
        // The card the person confirmed, never whichever card happens to be first now.
        await deny.mutateAsync({ id: to.cardId, note: message })
        setText("")
        setIntent(null)
        return true
      } catch (error) {
        setNotice(
          isApiError(error, 409)
            ? "That card was answered before your note went. Your text is still here."
            : `Couldn't send your note. ${error instanceof Error ? error.message : ""}`,
        )
        return false
      }
    }
    const ok = await sendFollowUp({
      text: withAttachments(message, options.attachments.map((file) => file.path)),
      draft_only: options.draftOnly || undefined,
    })
    if (ok) {
      setText("")
      setIntent(null)
    }
    return ok
  }

  const cardTitle = card ? capitalize(actionCopy(card.tool, card.action, look.nameOfSlug).ask) : ""
  const composer = card
    ? {
        placeholder:
          pending.length > 1
            ? `Reply to "${cardTitle}". Your note goes to ${leadName} with a no.`
            : `Reply to the card. Your note goes to ${leadName} with a no.`,
        disabled: false,
        mode: "note" as const,
      }
    : running
      ? { placeholder: `${leadName} is working. Stop the task to change course.`, disabled: true, mode: "running" as const }
      : teamGone
        ? { placeholder: "This task's team was deleted, so it takes no follow-up.", disabled: true, mode: "closed" as const }
        : rootIsLead
          ? { placeholder: `Give ${leadName} a follow-up`, disabled: false, mode: "follow-up" as const }
          : { placeholder: "Follow-ups go through the Lead. Start a new task.", disabled: true, mode: "closed" as const }

  const target: Target | null =
    composer.mode === "note" && card
      ? { kind: "note", cardId: card.id, title: cardTitle }
      : composer.mode === "follow-up"
        ? { kind: "follow-up" }
        : null

  const submit = async (message: string, options: SubmitOptions): Promise<boolean> => {
    if (!target) return false
    // A card can close, or another take its place, while the person types; a note goes only to
    // the card it was written for, and never becomes a turn, without an explicit yes.
    if (intent !== null && !sameTarget(intent, target)) {
      setSwap({ message, options, from: intent, to: target })
      return false
    }
    return send(message, options, target)
  }

  const demo = { card, approvals }
  const chips: Chip[] = []
  if (card) {
    chips.push({ kind: "question", id: "why" })
    const note = demoCardNote(summary?.scenario, demo)
    if (note) chips.push({ kind: "fill", text: note })
  }
  chips.push({ kind: "question", id: "so-far" }, { kind: "question", id: "who" })
  if (state.plan) chips.push({ kind: "question", id: "plan" })
  if (composer.mode === "follow-up") {
    for (const chip of demoFollowUps(summary?.scenario, demo)) chips.push({ kind: "follow-up", ...chip })
    if (latest?.outcome?.kind === "failed" || latest?.outcome?.kind === "cancelled") {
      chips.push({ kind: "follow-up", label: "Run it again", text: "Try again" })
    }
  }
  const visibleChips = chips.filter((chip) => !used.has(chipLabel(chip)))

  const markUsed = (label: string, on: boolean) =>
    setUsed((current) => {
      const next = new Set(current)
      if (on) next.add(label)
      else next.delete(label)
      return next
    })

  const pickChip = async (chip: Chip) => {
    const label = chipLabel(chip)
    if (chip.kind === "question") {
      setAnswers((current) => [...current, answerChip(chip.id, state, approvals, look)])
    } else if (chip.kind === "fill") {
      setText(chip.text)
      setIntent(target)
    } else {
      if (followUp.isPending) return
      // Hidden before the request, so a second click can't send it again; shown again on failure.
      markUsed(label, true)
      setNotice(null)
      const busy = "The task is still running. Pick this again once it finishes."
      if (!(await sendFollowUp({ text: chip.text, scenario: chip.scenario }, busy))) markUsed(label, false)
      return
    }
    markUsed(label, true)
  }

  // Follows new content while the reader is at the bottom; opens at the bottom.
  const scroller = useRef<HTMLDivElement>(null)
  const atBottom = useRef(true)
  const contentKey = `${state.calls.length}:${state.turns.length}:${approvals.length}:${answers.length}:${latest?.outcome?.kind}`
  useLayoutEffect(() => {
    const el = scroller.current
    if (el && atBottom.current) el.scrollTop = el.scrollHeight
  }, [contentKey])

  const title = root?.title ?? root?.task ?? summary?.title ?? ""
  useDocumentTitle(title)
  const hasWork = state.calls.some((call) => call.tool !== "plan")
  return (
    <div className="flex h-full flex-col">
      <div
        ref={scroller}
        onScroll={(event) => {
          const el = event.currentTarget
          atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48
        }}
        className="min-h-0 flex-1 overflow-y-auto"
      >
        <div className="mx-auto flex w-full max-w-[720px] flex-col gap-5 px-4 pt-6 pb-6 md:px-6 md:pt-8">
          <header className="flex flex-wrap items-center gap-3 pb-1">
            <h2 className="m-0 min-w-0 text-lg font-semibold tracking-tight">{title}</h2>
            {teamName && <TeamTag name={teamName} />}
            <span className="ml-auto text-sm text-muted-foreground tabular-nums">
              {summary?.scenario && state.costMicros === 0 ? "Free scripted demo" : `Cost ${money(state.costMicros)}`}
            </span>
            {status.text && <span className={cn("text-sm whitespace-nowrap", TONE_CLASS[status.tone])}>{status.text}</span>}
          </header>

          {state.taintSources.length > 0 ? (
            <TaintBanner sources={state.taintSources} />
          ) : hasWork ? (
            <div className="sticky top-2 z-[3] flex gap-3 rounded-lg bg-muted px-4 py-3 text-sm ring-1 ring-border">
              <CircleCheck className="mt-0.5 size-[18px] flex-none text-emerald-700 dark:text-emerald-400" />
              <div>
                <b className="font-semibold">Only trusted content so far.</b> Saves set to Allow run without asking you.
              </div>
            </div>
          ) : null}

          {state.turns.map((turn, index) => (
            <TurnSection
              key={turn.run_id}
              turn={turn}
              index={index}
              state={state}
              approvals={approvals}
              onCancel={stop}
              cancelling={cancel.isPending}
            />
          ))}

          {answers.map((answer, i) => (
            <RecordCard key={i} answer={answer} />
          ))}
        </div>
      </div>

      <div className="bg-linear-to-b from-transparent to-background to-[24px] pt-2 pb-4">
        <div className="mx-auto flex w-full max-w-[720px] flex-col px-4 md:px-6">
          {visibleChips.length > 0 && (
            <div className="flex gap-2 overflow-x-auto pt-1 pb-3 [scrollbar-width:none]">
              {visibleChips.map((chip) => (
                <button
                  key={chipLabel(chip)}
                  type="button"
                  onClick={() => void pickChip(chip)}
                  className="h-8 flex-none rounded-full bg-background px-3 text-sm whitespace-nowrap text-sidebar-foreground ring-1 ring-border ring-inset hover:text-foreground hover:ring-input"
                >
                  {chipLabel(chip)}
                </button>
              ))}
            </div>
          )}
          <Composer
            value={text}
            onChange={(next) => {
              if (!text.trim() && next.trim()) setIntent(target)
              if (!next.trim()) setIntent(null)
              setText(next)
              if (notice) setNotice(null)
              if (swap) setSwap(null)
            }}
            onSubmit={submit}
            onStop={stop}
            stopping={cancel.isPending}
            placeholder={composer.placeholder}
            label={composer.mode === "note" ? `Note to ${leadName} with a no` : `Message ${leadName}`}
            disabled={composer.disabled}
            sending={deny.isPending || followUp.isPending}
            action={composer.mode === "running" ? "stop" : "send"}
            modePicker={composer.mode === "follow-up"}
            attach={composer.mode === "follow-up" ? state.teamId : undefined}
          >
            {swap && (
              <div role="status" className="flex flex-wrap items-center gap-2 px-4 text-sm text-amber-800 dark:text-amber-300">
                {swap.to.kind === "follow-up"
                  ? "The card closed. Send this as a follow-up?"
                  : swap.from.kind === "note"
                    ? `That card closed. Send this no to "${swap.to.title}"?`
                    : `A card is waiting now. Send this as a no to "${swap.to.title}"?`}
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setIntent(swap.to)
                    void send(swap.message, swap.options, swap.to)
                  }}
                >
                  {swap.to.kind === "follow-up" ? "Send as a follow-up" : "Send as a no"}
                </Button>
              </div>
            )}
            {notice && (
              <p role="status" className="m-0 px-4 text-sm text-amber-800 dark:text-amber-300">
                {notice}
              </p>
            )}
          </Composer>
        </div>
      </div>
    </div>
  )
}

const SOURCE_KIND: Record<TaintSource["kind"], string> = {
  inbox: "Inbox",
  web: "Web",
  file: "Untrusted file",
  note: "Untrusted note",
}

function TaintBanner({ sources }: { sources: TaintSource[] }) {
  const chip = "rounded-full bg-card px-2.5 py-0.5 text-xs text-sidebar-foreground ring-1 ring-border ring-inset"
  return (
    // Sticky only from md: at phone width it would cover a sixth of the screen.
    <div className="z-[3] flex gap-3 rounded-lg bg-amber-50 md:sticky md:top-2 px-4 py-3 text-sm ring-1 ring-border dark:bg-card dark:bg-linear-to-r dark:from-amber-950/45 dark:to-amber-950/45">
      <TriangleAlert className="mt-0.5 size-[18px] flex-none text-amber-800 dark:text-amber-300" />
      <div className="min-w-0">
        <b className="font-semibold">This task read outside content.</b> {TAINT_EFFECT}
        <div className="mt-2 flex flex-wrap gap-2">
          {sources.map((source) => {
            const text = `${SOURCE_KIND[source.kind]}: ${
              source.kind === "inbox"
                ? source.message_id
                : source.kind === "web"
                  ? source.url
                  : source.kind === "file"
                    ? source.path
                    : `${source.scope} ${source.key}`
            }`
            // Only files have a page to open; the inbox has no endpoint and a URL is shown as text.
            return source.kind === "file" ? (
              <Link key={text} to={`/files?path=${encodeURIComponent(source.path)}`} className={cn(chip, "hover:text-foreground hover:ring-input")}>
                {text}
              </Link>
            ) : (
              <span key={text} className={cn(chip, "break-all")}>
                {text}
              </span>
            )
          })}
        </div>
      </div>
    </div>
  )
}

function TurnSection({
  turn,
  index,
  state,
  approvals,
  onCancel,
  cancelling,
}: {
  turn: Turn
  index: number
  state: ThreadState
  approvals: ApprovalView[]
  onCancel: () => void
  cancelling: boolean
}) {
  const look = useAgents(state.teamId)
  const run = state.runs.find((r) => r.id === turn.run_id)
  const lead = run ? look.byId(run.agent_id) : undefined
  const running = run?.status === "running"
  const plan = turnPlan(state, turn)
  const tree = callTree(state, turn.run_id)
  const actions = countCalls(tree)
  // Planning only until the turn's first call; a Lead that works without a plan is not planning.
  const planning = running && plan === null && actions === 0
  const now = useNow(1000, planning)
  const name = lead?.name ?? "Lead"
  const heading = planning
    ? `${name} is planning`
    : plan
      ? index === 0 ? `${name}'s plan` : "Follow-up plan"
      : index === 0 ? `${name}'s work` : "Follow-up"
  const cards = turnApprovals(state, approvals, turn)
  const waiting = cards.some((a) => a.status === "pending")
  const ending = turnEnding(turn)
  const elapsed = run ? Math.max(0, Math.floor((now - Date.parse(run.started_at)) / 1000)) : 0

  return (
    <>
      <div className="flex max-w-[85%] animate-in flex-col items-start gap-2 self-end rounded-xl bg-muted px-5 py-3 break-words whitespace-pre-wrap duration-300 fade-in slide-in-from-bottom-1">
        {turn.text}
      </div>

      {(plan || planning || actions > 0) && (
        <section className="overflow-hidden rounded-lg ring-1 ring-border">
          <header className="flex items-center gap-3 border-b border-border px-4 py-3 text-sm">
            {lead && <AgentShape icon={lead.icon} colour={lead.colour} pulse={planning} />}
            <b className="font-medium">{heading}</b>
            {!planning && !plan && <span className="text-sm text-muted-foreground">No plan recorded</span>}
            {turn.draft_only && (
              <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-sidebar-foreground">Draft only</span>
            )}
            {planning && <span className="ml-auto text-sm text-muted-foreground tabular-nums">{elapsed}s</span>}
          </header>
          {plan && (
            <ol className="m-0 flex list-none flex-col py-2 pl-0">
              {plan.map((step, i) => {
                const agent = step.agent === "self" ? lead : look.bySlug(step.agent)
                return (
                  <li key={i} className="grid grid-cols-[20px_minmax(0,1fr)] items-center gap-3 px-4 py-2 text-sm">
                    {agent ? <AgentShape icon={agent.icon} colour={agent.colour} /> : <span />}
                    <span>
                      <b className="font-medium">{step.agent === "self" ? name : look.nameOfSlug(step.agent)}</b>{" "}
                      {step.goal}
                    </span>
                  </li>
                )
              })}
            </ol>
          )}
          {actions > 0 && (
            // Keyed on running so it opens while the turn works and folds once it ends.
            <details key={running ? "open" : "closed"} open={running} className="group border-t border-border">
              <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-3 text-sm text-sidebar-foreground [&::-webkit-details-marker]:hidden">
                <ChevronDown className="size-4 transition-transform group-open:rotate-180" />
                What your team did, {actions} {actions === 1 ? "action" : "actions"}
              </summary>
              <div className="px-1 pb-2">
                <CallTree nodes={tree} />
              </div>
            </details>
          )}
          {running && (
            <footer className="flex justify-end px-2 pt-1 pb-2">
              <Button variant="ghost" size="sm" className="text-sidebar-foreground" disabled={cancelling} onClick={onCancel}>
                Cancel task
              </Button>
            </footer>
          )}
        </section>
      )}

      {cards.map((approval) => (
        <CardWithNote key={approval.id} approval={approval} />
      ))}

      {ending ? (
        <>
          <LeadMessage lead={lead}>{ending}</LeadMessage>
          {turn.outcome?.kind === "completed" && (
            <p className="m-0 pl-8 text-sm text-muted-foreground">{recordedOutcome(tree)}</p>
          )}
        </>
      ) : running && !waiting && !planning ? (
        <div className="flex items-center gap-3 text-sm text-muted-foreground">
          {lead && <AgentShape icon={lead.icon} colour={lead.colour} size="md" pulse />}
          <span>{name} is working</span>
        </div>
      ) : null}
    </>
  )
}

/** A card, then the note the person sent with a no, as their own message. */
function CardWithNote({ approval }: { approval: ApprovalView }) {
  const look = useAgents(approval.team_id)
  const copy = actionCopy(approval.tool, approval.action, look.nameOfSlug)
  return (
    <>
      <Slip approval={approval} />
      {approval.status === "denied" && approval.note && (
        <div className="flex max-w-[85%] flex-col items-end gap-1 self-end">
          <div className="rounded-xl bg-muted px-5 py-3 break-words whitespace-pre-wrap">{approval.note}</div>
          <small className="px-2 text-xs text-muted-foreground">Sent as a no to: {capitalize(copy.ask)}</small>
        </div>
      )}
    </>
  )
}

function LeadMessage({ lead, children }: { lead?: { icon: string; colour: string }; children: string }) {
  return (
    <div className="grid animate-in grid-cols-[20px_minmax(0,1fr)] gap-3 duration-300 fade-in slide-in-from-bottom-1">
      {lead ? <AgentShape icon={lead.icon} colour={lead.colour} size="md" className="mt-[5px]" /> : <span />}
      <p className="m-0 break-words whitespace-pre-wrap">{children}</p>
    </div>
  )
}

/** A chip's answer, built from the record and labelled as such, never as the Lead speaking. */
function RecordCard({ answer }: { answer: ChipAnswer }) {
  return (
    <div className="flex animate-in flex-col gap-2 rounded-lg bg-muted px-5 py-4 duration-300 fade-in slide-in-from-bottom-1">
      <div className="flex items-center gap-2 text-sm text-muted-foreground">
        <ScrollText className="size-4" />
        From the record: {answer.question}
      </div>
      {answer.lines.map((line, i) => (
        <p key={i} className="m-0 text-sm break-words">
          {line}
        </p>
      ))}
    </div>
  )
}
