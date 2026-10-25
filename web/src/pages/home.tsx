import { useState } from "react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { Link, useNavigate } from "react-router"
import { CircleCheck, Inbox, NotebookText, Target, type LucideIcon } from "lucide-react"
import { cn } from "cn"
import { approvalsQuery } from "@/api/approvals"
import { startDrill } from "@/api/drill"
import { healthQuery } from "@/api/meta"
import { runsQuery, startRun } from "@/api/runs"
import { Composer, type SubmitOptions } from "@/components/composer"
import { Slip } from "@/components/slip"
import { TaskRow } from "@/components/task-row"
import { useInvalidateOn } from "@/events/use-invalidate-on"
import { isApiError } from "@/lib/api"
import { withAttachments } from "@/lib/copy"
import { DEMO_STARTS, type DemoStart } from "@/lib/demo"

const DEMO_ICON: Record<DemoStart["id"], LucideIcon> = {
  s1: Inbox,
  clean: CircleCheck,
  launder: NotebookText,
  drill: Target,
}

export function HomePage() {
  const navigate = useNavigate()
  const health = useQuery(healthQuery)
  const runs = useQuery(runsQuery())
  const approvals = useQuery(approvalsQuery())
  useInvalidateOn(["approvals"], (event) => event.type === "approval_requested" || event.type === "approval_resolved")
  // A row's "Needs your OK" comes from the task's pending count, which changes on card events.
  useInvalidateOn(runsQuery().queryKey, (event) => event.type === "approval_requested" || event.type === "approval_resolved")

  const [text, setText] = useState("")
  const [error, setError] = useState<string | null>(null)

  const start = useMutation({
    mutationFn: async (demo: DemoStart | { task: string; draftOnly: boolean }) => {
      if ("id" in demo) {
        if (demo.id === "drill") return startDrill()
        return startRun({ task: demo.task ?? "", title: demo.label, scenario: demo.scenario })
      }
      return startRun({ task: demo.task, draft_only: demo.draftOnly || undefined })
    },
  })

  const submit = async (message: string, options: SubmitOptions): Promise<boolean> => {
    setError(null)
    try {
      const { run_id } = await start.mutateAsync({
        task: withAttachments(message, options.attachments.map((file) => file.path)),
        draftOnly: options.draftOnly,
      })
      setText("")
      void navigate(`/tasks/${run_id}`)
      return true
    } catch (failure) {
      setError(
        isApiError(failure) && failure.field === "task"
          ? failure.message
          : `Couldn't start the task. ${failure instanceof Error ? failure.message : ""}`,
      )
      return false
    }
  }

  const runDemo = async (demo: DemoStart) => {
    setError(null)
    try {
      const { run_id } = await start.mutateAsync(demo)
      void navigate(`/tasks/${run_id}`)
    } catch (failure) {
      if (demo.conflict && isApiError(failure, 409)) {
        setError(demo.conflict)
      } else {
        setError(`Couldn't start the demo. ${failure instanceof Error ? failure.message : ""}`)
      }
    }
  }

  const pending = (approvals.data?.approvals ?? [])
    .filter((a) => a.status === "pending")
    .sort((a, b) => a.created_at.localeCompare(b.created_at))
  const windowMinutes = pending[0]
    ? Math.round((Date.parse(pending[0].expires_at) - Date.parse(pending[0].created_at)) / 60_000)
    : null
  const recent = (runs.data?.runs ?? []).slice(0, 4)
  const calm = pending.length === 0

  return (
    <div
      className={cn(
        "mx-auto flex min-h-full w-full max-w-[720px] flex-col gap-6 px-4 pt-6 pb-16 md:gap-8 md:px-6 md:pt-12",
        calm && "justify-center pb-[12vh]",
      )}
    >
      {pending.length > 0 && (
        <section aria-labelledby="home-needs">
          <h2 id="home-needs" className="m-0 mb-3 flex items-center gap-2 text-sm font-medium text-sidebar-foreground">
            Needs your OK
            <span className="grid h-5 min-w-5 place-items-center rounded-full bg-amber-100 px-1.5 text-xs font-semibold text-amber-800 tabular-nums dark:bg-amber-950 dark:text-amber-300">
              {pending.length}
            </span>
            {windowMinutes !== null && (
              <span className="ml-auto font-normal text-muted-foreground">
                No answer in {windowMinutes} minutes counts as no
              </span>
            )}
          </h2>
          <div className="flex flex-col gap-3">
            {pending.map((approval) => (
              <Slip key={approval.id} approval={approval} />
            ))}
          </div>
        </section>
      )}

      <section className="flex flex-col gap-5" aria-label="New task">
        <h1 className="m-0 text-center text-[26px] leading-tight font-medium tracking-tight text-balance md:text-3xl">
          What should your team do?
        </h1>
        <Composer
          value={text}
          onChange={(next) => {
            setText(next)
            if (error) setError(null)
          }}
          onSubmit={submit}
          placeholder="Describe a task, like you would to a colleague"
          label="New task"
          sending={start.isPending}
          modePicker
          attach
          menuBelow
          autoFocus
        >
          {error && (
            <p role="alert" className="m-0 px-4 text-sm text-destructive">
              {error}
            </p>
          )}
        </Composer>
        <p className="m-0 text-center text-sm text-muted-foreground">
          Lead plans it and hands it out. Your team asks you before sending, before opening a site not on your list,
          and before saving once a task has read outside content.
          {health.data && (
            <>
              {" "}
              {health.data.provider === "scripted"
                ? "No API key: free-text tasks get a scripted reply."
                : "Free-text tasks run on the live model and cost money; the demos below are free."}
            </>
          )}
        </p>
        <div className="flex flex-wrap items-center justify-center gap-2">
          <span className="mr-1 text-sm text-muted-foreground">Free scripted demos</span>
          {DEMO_STARTS.map((demo) => {
            const Icon = DEMO_ICON[demo.id]
            return (
              <button
                key={demo.id}
                type="button"
                title={demo.detail}
                disabled={start.isPending}
                onClick={() => void runDemo(demo)}
                className="inline-flex h-8 items-center gap-2 rounded-full px-3 text-sm text-sidebar-foreground ring-1 ring-border ring-inset hover:text-foreground hover:ring-input disabled:opacity-60"
              >
                <Icon className="size-4" />
                {demo.label}
              </button>
            )
          })}
        </div>
      </section>

      {recent.length > 0 && (
        <section aria-labelledby="home-recent">
          <h2 id="home-recent" className="m-0 mb-3 flex items-center gap-2 text-sm font-medium text-sidebar-foreground">
            Recent
            <Link
              to="/tasks"
              className="ml-auto text-sm font-normal text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground hover:decoration-current"
            >
              All tasks
            </Link>
          </h2>
          <div className="flex flex-col">
            {recent.map((run) => (
              <TaskRow key={run.id} run={run} />
            ))}
          </div>
        </section>
      )}
    </div>
  )
}
