import { useRef, useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "react-router"
import { ChevronDown, Plus, Users } from "lucide-react"
import { createTeam } from "@/api/teams"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { isApiError } from "@/lib/api"
import { selectTeam, useSelectedTeam, useTeamNames } from "@/lib/selected-team"

/** The selected team above the navigation: switch, create, or open a deleted team's notes. */
export function TeamSwitcher({ onNavigate }: { onNavigate?: () => void }) {
  const { team, teams } = useSelectedTeam()
  const { teams: all } = useTeamNames()
  const deleted = all.filter((t) => t.deleted_at !== null)
  const navigate = useNavigate()
  const client = useQueryClient()
  const [adding, setAdding] = useState(false)
  const trigger = useRef<HTMLButtonElement>(null)
  const nameInput = useRef<HTMLInputElement>(null)
  // Radix returns focus to the trigger when the menu closes; after "New team" it goes to the field.
  const openingForm = useRef(false)
  const [name, setName] = useState("")
  const [error, setError] = useState<string | null>(null)

  const create = useMutation({
    mutationFn: () => createTeam({ name }),
    onSuccess: async (created) => {
      // Team writes emit no events, so the lists refetch here.
      await client.invalidateQueries({ queryKey: ["teams"] })
      await client.invalidateQueries({ queryKey: ["agents"] })
      selectTeam(created.id)
      setAdding(false)
      setName("")
      onNavigate?.()
      void navigate("/team")
    },
    onError: (failure) =>
      setError(isApiError(failure, 422) ? failure.message : `Couldn't create the team. ${failure.message}`),
  })

  if (!team) return null
  return (
    <div className="flex flex-col gap-2 pb-3">
      <DropdownMenu>
        <DropdownMenuTrigger
          ref={trigger}
          className="flex h-10 w-full items-center gap-2 rounded-md px-3 text-left text-sm ring-1 ring-border ring-inset outline-none hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring">
          <Users className="size-4 flex-none text-sidebar-foreground" />
          <span className="min-w-0 flex-1 truncate font-medium">{team.name}</span>
          <ChevronDown className="size-4 flex-none text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="start"
          className="w-[232px] max-w-[calc(100vw-48px)]"
          onCloseAutoFocus={(event) => {
            if (!openingForm.current) return
            openingForm.current = false
            event.preventDefault()
            requestAnimationFrame(() => nameInput.current?.focus())
          }}
        >
          <DropdownMenuLabel className="text-xs text-muted-foreground">Teams</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={team.id} onValueChange={(id) => selectTeam(id)}>
            {teams.map((t) => (
              <DropdownMenuRadioItem key={t.id} value={t.id} className="min-w-0">
                <span className="truncate">{t.name}</span>
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onSelect={() => {
              setError(null)
              openingForm.current = true
              setAdding(true)
            }}
          >
            <Plus className="size-4" />
            New team
          </DropdownMenuItem>
          {deleted.length > 0 && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel className="text-xs text-muted-foreground">Deleted teams, notes only</DropdownMenuLabel>
              {deleted.map((t) => (
                <DropdownMenuItem
                  key={t.id}
                  onSelect={() => {
                    onNavigate?.()
                    void navigate(`/notes?deleted=${t.id}`)
                  }}
                >
                  <span className="truncate text-sidebar-foreground">{t.name}</span>
                </DropdownMenuItem>
              ))}
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
      {adding && (
        <form
          className="flex flex-col gap-2 px-1"
          onSubmit={(event) => {
            event.preventDefault()
            if (name.trim() && !create.isPending) create.mutate()
          }}
        >
          <input
            ref={nameInput}
            aria-label="New team name"
            placeholder="Team name"
            value={name}
            maxLength={60}
            onChange={(event) => {
              setName(event.target.value)
              setError(null)
            }}
            className="h-9 rounded-md bg-card px-3 text-sm ring-1 ring-border outline-none ring-inset focus-visible:ring-2 focus-visible:ring-ring"
          />
          {error && (
            <p role="alert" className="m-0 text-sm text-destructive">
              {error}
            </p>
          )}
          <div className="flex gap-2">
            <Button type="submit" size="sm" disabled={!name.trim() || create.isPending}>
              Create
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                setAdding(false)
                trigger.current?.focus()
              }}
            >
              Cancel
            </Button>
          </div>
          <p className="m-0 text-xs text-muted-foreground">It starts with a Lead. Add teammates on Your team.</p>
        </form>
      )}
    </div>
  )
}
