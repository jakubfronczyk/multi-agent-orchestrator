import { useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "react-router"
import { Folder, NotebookText, Users } from "lucide-react"
import { createTeam, refetchTeams } from "@/api/teams"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { isApiError } from "@/lib/api"
import { setNewTeamOpen, useNewTeamOpen } from "@/lib/new-team"
import { selectTeam } from "@/lib/selected-team"

const GETS: [typeof Users, string, string][] = [
  [Users, "A Lead", "Plans each task. You add the teammates it hands work to."],
  [Folder, "A folder", "Other teams' agents open it only if you grant them access."],
  [NotebookText, "Notes and history", "What it learns and every task it ran."],
]

/** Names a new team, selects it, and opens Your team with the first teammate's editor. */
export function NewTeamDialog() {
  const open = useNewTeamOpen()
  const navigate = useNavigate()
  const client = useQueryClient()
  const [name, setName] = useState("")
  const [error, setError] = useState<string | null>(null)

  const create = useMutation({
    mutationFn: () => createTeam({ name: name.trim() }),
    onSuccess: async (created) => {
      await refetchTeams(client)
      selectTeam(created.id)
      setNewTeamOpen(false)
      setName("")
      void navigate("/team?add=1")
    },
    onError: (failure) =>
      setError(isApiError(failure, 422) ? failure.message : `Couldn't create the team. ${failure.message}`),
  })

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setError(null)
        setNewTeamOpen(next)
      }}
    >
      <DialogContent className="sm:max-w-xl">
        <form
          className="flex flex-col gap-6"
          onSubmit={(event) => {
            event.preventDefault()
            if (name.trim() && !create.isPending) create.mutate()
          }}
        >
          <DialogHeader>
            <DialogTitle className="text-lg">Build your own team</DialogTitle>
            <DialogDescription>A team gets its own Lead, folder and notes. Name it, then add its first teammate.</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-2">
            <label htmlFor="new-team-name" className="text-sm font-medium">
              Team name
            </label>
            <input
              id="new-team-name"
              autoFocus
              placeholder="Support desk"
              value={name}
              maxLength={60}
              onChange={(event) => {
                setName(event.target.value)
                setError(null)
              }}
              className="h-11 rounded-lg bg-card px-3.5 text-base ring-1 ring-border outline-none ring-inset placeholder:text-muted-foreground/60 focus-visible:ring-2 focus-visible:ring-ring"
            />
            {error && (
              <p role="alert" className="m-0 text-sm text-destructive">
                {error}
              </p>
            )}
          </div>

          <ul className="m-0 grid list-none grid-cols-1 gap-2 p-0 sm:grid-cols-3">
            {GETS.map(([Icon, title, text]) => (
              <li key={title} className="flex flex-col gap-1.5 rounded-lg bg-muted px-3.5 py-3">
                <Icon className="size-4 text-muted-foreground" />
                <b className="text-sm font-medium">{title}</b>
                <p className="m-0 text-xs/relaxed text-muted-foreground">{text}</p>
              </li>
            ))}
          </ul>

          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => setNewTeamOpen(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!name.trim() || create.isPending}>
              Create team
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
