import { useState } from "react"
import { useMutation, useQuery } from "@tanstack/react-query"
import { Link, useNavigate } from "react-router"
import { ArrowRight } from "lucide-react"
import { startRun } from "@/api/runs"
import { teamsQuery } from "@/api/teams"
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import type { TeamView } from "@/generated/TeamView"
import { selectTeam } from "@/lib/selected-team"
import { setWelcomeOpen, STARTERS, useWelcomeOpen, type Starter } from "@/lib/welcome"

/** Shown on a browser's first visit and from the sidebar's "?"; each card starts a task on its team. */
export function WelcomeDialog() {
  const open = useWelcomeOpen()
  const navigate = useNavigate()
  const teams = useQuery(teamsQuery())
  const [error, setError] = useState<string | null>(null)

  const cards = STARTERS.flatMap((starter) => {
    const team = teams.data?.teams.find((t) => t.name === starter.team)
    return team ? [{ starter, team }] : []
  })

  const start = useMutation({
    mutationFn: ({ starter, team }: { starter: Starter; team: TeamView }) =>
      startRun({ task: starter.prompt, team_id: team.id }),
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
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>Welcome to Orchestrator</DialogTitle>
          <DialogDescription asChild>
            <div className="flex flex-col gap-1">
              <p>You give a team of agents a task in plain words, and they do it.</p>
              <p>A Lead plans the steps and hands each one to a teammate who reads, writes or sends.</p>
              <p>Before anything risky, like sending a message or saving after reading outside content, it asks you first.</p>
            </div>
          </DialogDescription>
        </DialogHeader>

        {cards.length > 0 && (
          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {cards.map(({ starter, team }) => (
              <li key={team.id} className="rounded-lg border bg-card p-3">
                <div className="text-sm font-medium text-card-foreground">{team.name}</div>
                <p className="m-0 mt-0.5 text-sm text-muted-foreground">{starter.about}</p>
                <button
                  type="button"
                  disabled={start.isPending}
                  onClick={() => {
                    setError(null)
                    start.mutate({ starter, team })
                  }}
                  className="mt-2 flex w-full items-center gap-2 rounded-md bg-secondary px-3 py-2 text-left text-sm text-secondary-foreground hover:bg-accent hover:text-accent-foreground disabled:opacity-50"
                >
                  <span className="min-w-0 flex-1">“{starter.prompt}”</span>
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

        <Link
          to="/team"
          onClick={() => setWelcomeOpen(false)}
          className="text-sm font-medium text-primary underline-offset-4 hover:underline"
        >
          Build your own team
        </Link>
      </DialogContent>
    </Dialog>
  )
}
