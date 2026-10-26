import { useSyncExternalStore } from "react"

const listeners = new Set<() => void>()
let open = false

/** Opens or closes the new team dialog from anywhere: the team switcher or the welcome dialog. */
export function setNewTeamOpen(next: boolean) {
  open = next
  for (const listener of listeners) listener()
}

export function useNewTeamOpen(): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => open,
  )
}
