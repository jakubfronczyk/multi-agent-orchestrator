import { useSyncExternalStore } from "react"

const KEY = "welcome-seen"
const listeners = new Set<() => void>()

// Per browser; storage can be missing or throw (private windows), so every access is guarded.
function seen(): boolean {
  try {
    return window.localStorage.getItem(KEY) !== null
  } catch {
    return false
  }
}

let open = !seen()

/** Opens the welcome dialog; closing it marks it seen, so it never opens by itself again. */
export function setWelcomeOpen(next: boolean) {
  open = next
  if (!next) {
    try {
      window.localStorage.setItem(KEY, "1")
    } catch {
      // Without storage it shows again on the next visit.
    }
  }
  for (const listener of listeners) listener()
}

export function useWelcomeOpen(): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => open,
  )
}

export interface Starter {
  /** Matched against the server's team names; a team that is not there gets no card. */
  team: string
  about: string
  prompt: string
}

/** The teams the server seeds on first boot, each with a task that shows what it does. */
export const STARTERS: Starter[] = [
  {
    team: "Access desk",
    about: "Checks access requests against the access policy and tells the owner.",
    prompt: "jana sent an access request, check it and let the owner know what we decided",
  },
  {
    team: "Finance desk",
    about: "Checks supplier invoices against the payment policy; never pays.",
    prompt: "ok approve whatever we're allowed to and let accounts know",
  },
  {
    team: "HR desk",
    about: "Onboards new hires: start dates, managers, welcome emails.",
    prompt: "can u send ola a welcome mail with the first day checklist",
  },
  {
    team: "Research desk",
    about: "Reads web pages and writes short, sourced briefs.",
    prompt: "which tlds does rfc 2606 reserve? its at https://www.rfc-editor.org/rfc/rfc2606",
  },
]
