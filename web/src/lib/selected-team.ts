import { useSyncExternalStore } from "react"
import { useQuery } from "@tanstack/react-query"
import { teamsQuery } from "@/api/teams"
import type { TeamView } from "@/generated/TeamView"

const KEY = "selected-team"
const listeners = new Set<() => void>()

// Per viewer; storage can be missing or throw (private windows), so every access is guarded.
function read(): string | null {
  try {
    return window.localStorage.getItem(KEY)
  } catch {
    return null
  }
}

let stored = read()

export function selectTeam(id: string) {
  stored = id
  try {
    window.localStorage.setItem(KEY, id)
  } catch {
    // The choice still holds for this tab.
  }
  for (const listener of listeners) listener()
}

/**
 * The team the setup pages show: the stored choice while it names a live team, else the oldest
 * live team. `team` is undefined only until the list loads.
 */
export function useSelectedTeam(): { team: TeamView | undefined; teams: TeamView[] } {
  const id = useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => stored,
  )
  const { data } = useQuery(teamsQuery())
  const teams = data?.teams ?? []
  const team = teams.find((t) => t.id === id) ?? teams[0]
  return { team, teams }
}

/** Team names by id, deleted teams included, for labels on history. */
export function useTeamNames() {
  const { data } = useQuery(teamsQuery(true))
  const teams = data?.teams ?? []
  const live = teams.filter((t) => t.deleted_at === null)
  const nameOf = (id: string) => {
    const team = teams.find((t) => t.id === id)
    if (!team) return undefined
    return team.deleted_at === null ? team.name : `${team.name} (deleted team)`
  }
  return { nameOf, isDeleted: (id: string) => teams.find((t) => t.id === id)?.deleted_at != null, many: live.length > 1 || teams.length > live.length, teams }
}
