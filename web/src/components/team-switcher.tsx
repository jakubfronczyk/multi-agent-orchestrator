import { useNavigate } from "react-router"
import { ChevronDown, Plus, Users } from "lucide-react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { setNewTeamOpen } from "@/lib/new-team"
import { selectTeam, useSelectedTeam, useTeamNames } from "@/lib/selected-team"

/** The selected team above the navigation: switch, open the new team dialog, or open a deleted team's tasks or notes. */
/** How many deleted teams the menu lists, newest first. */
const DELETED_SHOWN = 5

export function TeamSwitcher({ onNavigate }: { onNavigate?: () => void }) {
  const { team, teams } = useSelectedTeam()
  const { teams: all } = useTeamNames()
  const deleted = all.filter((t) => t.deleted_at !== null)
  // The menu keeps the most recently deleted few; the rest stay reachable from Tasks and Notes.
  const recent = [...deleted].sort((a, b) => (b.deleted_at ?? "").localeCompare(a.deleted_at ?? "")).slice(0, DELETED_SHOWN)
  const navigate = useNavigate()

  if (!team) return null
  return (
    <div className="pb-3">
      <DropdownMenu>
        <DropdownMenuTrigger
          className="flex h-10 w-full items-center gap-2 rounded-md px-3 text-left text-sm ring-1 ring-border ring-inset outline-none hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring">
          <Users className="size-4 flex-none text-sidebar-foreground" />
          <span className="min-w-0 flex-1 truncate font-medium">{team.name}</span>
          <ChevronDown className="size-4 flex-none text-muted-foreground" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-[232px] max-w-[calc(100vw-48px)]">
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
              onNavigate?.()
              setNewTeamOpen(true)
            }}
          >
            <Plus className="size-4" />
            New team
          </DropdownMenuItem>
          {deleted.length > 0 && (
            <>
              <DropdownMenuSeparator />
              <DropdownMenuLabel className="text-xs text-muted-foreground">
                {deleted.length > DELETED_SHOWN
                  ? `Deleted teams, read-only: latest ${DELETED_SHOWN} of ${deleted.length}`
                  : "Deleted teams, read-only"}
              </DropdownMenuLabel>
              {recent.map((t) => (
                <DropdownMenuSub key={t.id}>
                  <DropdownMenuSubTrigger className="min-w-0">
                    <span className="truncate text-sidebar-foreground">{t.name}</span>
                  </DropdownMenuSubTrigger>
                  <DropdownMenuSubContent>
                    {(["tasks", "notes"] as const).map((page) => (
                      <DropdownMenuItem
                        key={page}
                        onSelect={() => {
                          onNavigate?.()
                          void navigate(`/${page}?deleted=${t.id}`)
                        }}
                      >
                        {page === "tasks" ? "Tasks" : "Notes"}
                      </DropdownMenuItem>
                    ))}
                  </DropdownMenuSubContent>
                </DropdownMenuSub>
              ))}
            </>
          )}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}
