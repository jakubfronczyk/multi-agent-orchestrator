import { useLayoutEffect, useRef } from "react"
import { useQuery } from "@tanstack/react-query"
import { Link, useLocation, useSearchParams } from "react-router"
import { Check, FlaskConical, House, Shield, X } from "lucide-react"
import { approvalsQuery } from "@/api/approvals"
import { toolsQuery } from "@/api/meta"
import { runQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { Button } from "@/components/ui/button"
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import { useAgents } from "@/lib/agents"
import { explainApproval, explainCall, type Explanation, type Hop } from "@/lib/how-this-happened"
import { isApiError } from "@/lib/api"
import { dayTime } from "@/lib/time"

/**
 * The "How this happened" sheet for one call, opened from anywhere through the URL. Focus goes
 * to the close button on open and back to whatever opened it on close.
 */
export function HowThisHappenedSheet() {
  const [params, setParams] = useSearchParams()
  const callId = params.get("call")
  const rootRunId = params.get("task")
  const open = callId !== null && rootRunId !== null
  const opener = useRef<HTMLElement | null>(null)
  const closeButton = useRef<HTMLButtonElement>(null)

  // Runs before Radix moves focus into the sheet, so the opener is still the active element.
  useLayoutEffect(() => {
    if (open && document.activeElement instanceof HTMLElement && document.activeElement !== document.body) {
      opener.current = document.activeElement
    }
  }, [open])

  const close = () =>
    setParams((current) => {
      const next = new URLSearchParams(current)
      next.delete("call")
      next.delete("task")
      return next
    })

  return (
    <Sheet open={open} onOpenChange={(next) => !next && close()}>
      <SheetContent
        side="right"
        showCloseButton={false}
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          closeButton.current?.focus()
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          opener.current?.focus()
          opener.current = null
        }}
        className="gap-5 overflow-y-auto bg-card px-6 pt-5 pb-8 shadow-lg data-[side=right]:w-full data-[side=right]:max-w-[440px] data-[side=right]:sm:max-w-[440px]"
      >
        <div className="flex items-center gap-2">
          <SheetTitle className="text-lg font-semibold">How this happened</SheetTitle>
          <Button
            ref={closeButton}
            variant="ghost"
            size="icon"
            aria-label="Close"
            className="ml-auto text-sidebar-foreground"
            onClick={close}
          >
            <X className="size-[18px]" />
          </Button>
        </div>
        {open && <SheetBody callId={callId} rootRunId={rootRunId} />}
      </SheetContent>
    </Sheet>
  )
}

function SheetBody({ callId, rootRunId }: { callId: string; rootRunId: string }) {
  const look = useAgents()
  const detail = useQuery(runQuery(rootRunId))
  const approvals = useQuery(approvalsQuery())
  const tools = useQuery(toolsQuery)

  useInvalidateOn(runQuery(rootRunId).queryKey, (event) => event.root_run_id === rootRunId)
  useInvalidateOn(
    approvalsQuery().queryKey,
    (event) => event.type === "approval_requested" || event.type === "approval_resolved",
  )

  const approval = approvals.data?.approvals.find((a) => a.tool_call_id === callId)
  const call = detail.data?.tool_calls.find((c) => c.id === callId)
  const explanation = approval
    ? explainApproval(approval, detail.data, look)
    : call && detail.data
      ? explainCall(call, detail.data, tools.data, look)
      : undefined

  if (explanation) return <ExplanationView explanation={explanation} />
  const missing = isApiError(detail.error, 404) || (detail.data && approvals.data && !call)
  return (
    <>
      <SheetDescription className="sr-only">Details of one call</SheetDescription>
      {missing ? (
        <p className="m-0 text-sm text-muted-foreground">This call isn't part of that task.</p>
      ) : detail.error || approvals.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load this call. {detail.error?.message ?? approvals.error?.message}</p>
      ) : null}
    </>
  )
}

function ExplanationView({ explanation }: { explanation: Explanation }) {
  const location = useLocation()
  const inItsTask = location.pathname === `/tasks/${explanation.rootRunId}`
  return (
    <>
      <div className="grid grid-cols-[20px_minmax(0,1fr)] gap-3">
        {explanation.agent ? (
          <AgentShape icon={explanation.agent.icon} colour={explanation.agent.colour} size="lg" className="mt-0.5" />
        ) : (
          <span />
        )}
        <div className="min-w-0">
          <b className="block font-medium break-words">{explanation.summary}</b>
          <SheetDescription className="text-sm text-muted-foreground">
            {explanation.byline}, {dayTime(explanation.at)}
          </SheetDescription>
        </div>
      </div>

      <ol className="m-0 flex list-none flex-col p-0">
        {explanation.hops.map((hop) => (
          <HopRow key={hop.kind} hop={hop} />
        ))}
      </ol>

      <details className="group">
        <summary className="cursor-pointer text-sm text-muted-foreground">Technical details</summary>
        <pre className="mt-2 mb-0 rounded-md bg-muted p-3 font-mono text-xs/relaxed break-words whitespace-pre-wrap text-sidebar-foreground">
          {JSON.stringify(explanation.technical, null, 2)}
        </pre>
      </details>

      {!inItsTask && (
        <Button variant="outline" asChild className="self-start">
          <Link to={`/tasks/${explanation.rootRunId}`}>Open the task</Link>
        </Button>
      )}
    </>
  )
}

function HopIcon({ hop }: { hop: Hop }) {
  switch (hop.kind) {
    case "you":
      return (
        <svg viewBox="0 0 16 16" aria-hidden="true" className="size-4">
          <circle cx="8" cy="8" r="6.2" fill="none" stroke="currentColor" strokeWidth="2" />
        </svg>
      )
    case "handoff":
      return hop.agent ? <AgentShape icon={hop.agent.icon} colour={hop.agent.colour} size="md" /> : null
    case "read":
      return <House className="size-4" />
    case "rule":
      return <FlaskConical className="size-4" />
    case "decided":
      return hop.stopped ? <Shield className="size-4" /> : <Check className="size-4" />
  }
}

function HopRow({ hop }: { hop: Hop }) {
  return (
    <li className="relative grid grid-cols-[20px_minmax(0,1fr)] gap-3 pb-4 text-sm before:absolute before:top-[22px] before:bottom-0.5 before:left-[9.5px] before:border-l before:border-dashed before:border-input last:pb-0 last:before:hidden">
      <span className="grid size-5 place-items-center text-muted-foreground">
        <HopIcon hop={hop} />
      </span>
      <div className="min-w-0">
        <small className="mb-0.5 block text-xs tracking-wide text-muted-foreground">{hop.label}</small>
        {hop.lines.map((line, i) => (
          <p key={i} className="m-0 break-words whitespace-pre-line [&+p]:mt-1">
            {hop.quoted ? `"${line.trim()}"` : line}
          </p>
        ))}
        {hop.note && <p className="mt-1 mb-0 text-muted-foreground">{hop.note}</p>}
      </div>
    </li>
  )
}
