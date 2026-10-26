import { useState } from "react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { useNavigate } from "react-router"
import { ArrowRight, MessageSquare, Plus, ShieldCheck, Workflow, type LucideIcon } from "lucide-react"
import { startRun } from "@/api/runs"
import { teamsQuery } from "@/api/teams"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import type { TeamView } from "@/generated/TeamView"
import { setNewTeamOpen } from "@/lib/new-team"
import { selectTeam } from "@/lib/selected-team"
import { setWelcomeHidden, setWelcomeOpen, STARTERS, useWelcomeOpen, welcomeHidden, type Starter } from "@/lib/welcome"

const STEPS: [LucideIcon, string, string][] = [
  [MessageSquare, "You ask", "Describe a task the way you'd tell a colleague."],
  [Workflow, "The Lead plans", "It hands each step to a teammate who reads, writes or sends."],
  [ShieldCheck, "You approve", "Sending, or saving after reading outside content, waits for your yes."],
]

/** Opens on every visit until "Don't show this again" is ticked, and always from the sidebar's "?"; each card starts a task on its team. */
export function WelcomeDialog() {
  const open = useWelcomeOpen()
  const navigate = useNavigate()
  const teams = useQuery(teamsQuery())
  const [error, setError] = useState<string | null>(null)
  // Read when the component mounts, so the box shows what this browser already chose.
  const [hidden, setHidden] = useState(welcomeHidden)

  const cards = STARTERS.flatMap((starter) => {
    const team = teams.data?.teams.find((t) => t.name === starter.team)
    return team ? [{ starter, team }] : []
  })

  const start = useMutation({
    mutationFn: ({ starter, team }: { starter: Starter; team: TeamView }) =>
      startRun({ task: starter.tasks[0].prompt, team_id: team.id }),
    onSuccess: ({ run_id }, { team }) => {
      selectTeam(team.id)
      setWelcomeOpen(false)
      void navigate(`/tasks/${run_id}`)
    },
    onError: (failure) => setError(`Couldn't start the task. ${failure instanceof Error ? failure.message : ""}`),
  })

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setError(null)
        setWelcomeOpen(next)
      }}
    >
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle className="text-xl">Welcome to Orchestrator</DialogTitle>
          <DialogDescription>A team of agents that does tasks you describe in plain words.</DialogDescription>
        </DialogHeader>

        <ol className="m-0 grid list-none grid-cols-1 gap-3 p-0 sm:grid-cols-3">
          {STEPS.map(([Icon, title, text], i) => (
            <li key={title} className="flex gap-3">
              <span className="grid size-8 flex-none place-items-center rounded-full bg-secondary text-secondary-foreground">
                <Icon className="size-4" aria-hidden />
              </span>
              <span className="flex min-w-0 flex-col">
                <b className="text-sm font-medium">
                  {i + 1}. {title}
                </b>
                <span className="text-xs/relaxed text-muted-foreground">{text}</span>
              </span>
            </li>
          ))}
        </ol>

        {cards.length > 0 && (
          <ul className="m-0 grid list-none grid-cols-1 gap-3 p-0 sm:grid-cols-2">
            {cards.map(({ starter, team }) => (
              <li key={team.id} className="flex flex-col rounded-lg border bg-card p-4">
                <div className="text-sm font-medium text-card-foreground">{team.name}</div>
                <p className="m-0 mt-0.5 mb-3 text-sm text-muted-foreground">{starter.about}</p>
                <button
                  type="button"
                  disabled={start.isPending}
                  onClick={() => {
                    setError(null)
                    start.mutate({ starter, team })
                  }}
                  className="mt-auto flex w-full items-center gap-2 rounded-md bg-secondary px-3 py-2 text-left text-sm text-secondary-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-50"
                >
                  <span className="min-w-0 flex-1">“{starter.tasks[0].prompt}”</span>
                  <ArrowRight className="size-4 flex-none" aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}

        {error && (
          <p role="alert" className="m-0 text-sm text-destructive">
            {error}
          </p>
        )}

        <button
          type="button"
          onClick={() => {
            setWelcomeOpen(false)
            setNewTeamOpen(true)
          }}
          className="group flex w-full items-center gap-3 rounded-lg border border-dashed p-3 text-left hover:border-solid hover:bg-accent"
        >
          <span className="grid size-9 flex-none place-items-center rounded-md bg-secondary text-secondary-foreground">
            <Plus className="size-4" aria-hidden />
          </span>
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="text-sm font-medium">Build your own team</span>
            <span className="text-sm text-muted-foreground">Name it, then add the teammates it needs.</span>
          </span>
          <ArrowRight className="size-4 flex-none text-muted-foreground group-hover:text-foreground" aria-hidden />
        </button>

        <div className="flex flex-wrap items-center justify-between gap-3 border-t pt-3">
          <label className="flex items-center gap-2 text-sm text-muted-foreground">
            <input
              type="checkbox"
              checked={hidden}
              onChange={(event) => {
                setHidden(event.target.checked)
                setWelcomeHidden(event.target.checked)
              }}
              className="size-4 accent-primary"
            />
            Don't show this again
          </label>
          <button
            type="button"
            onClick={() => setWelcomeOpen(false)}
            className="rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground hover:bg-primary/90"
          >
            Done
          </button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
