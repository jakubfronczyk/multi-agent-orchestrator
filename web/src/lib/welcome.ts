import { useSyncExternalStore } from "react"

const KEY = "welcome-hidden"
const listeners = new Set<() => void>()

// Per browser; storage can be missing or throw (private windows), so every access is guarded.
export function welcomeHidden(): boolean {
  try {
    return window.localStorage.getItem(KEY) !== null
  } catch {
    return false
  }
}

/** Stops the dialog opening by itself on each visit, or lets it open again; the "?" always opens it. */
export function setWelcomeHidden(hidden: boolean) {
  try {
    if (hidden) window.localStorage.setItem(KEY, "1")
    else window.localStorage.removeItem(KEY)
  } catch {
    // Without storage it opens again on the next visit.
  }
}

let open = !welcomeHidden()

/** Opens or closes the welcome dialog for this page load; closing remembers nothing. */
export function setWelcomeOpen(next: boolean) {
  open = next
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

export interface StarterTask {
  label: string
  prompt: string
}

export interface Starter {
  /** Matched against the server's team names; a team that is not there gets no card. */
  team: string
  about: string
  /** The first is the welcome card's; Home offers them all. */
  tasks: [StarterTask, ...StarterTask[]]
}

/** The teams the server seeds on first boot, each with tasks written for its files. */
export const STARTERS: Starter[] = [
  {
    team: "Access desk",
    about: "Checks access requests against the access policy and tells the owner.",
    tasks: [
      { label: "Answer Jana's request", prompt: "jana sent an access request, check it and let the owner know what we decided" },
      { label: "Grants breaking policy", prompt: "can u check our access grants - is anything breaking policy?" },
      { label: "Who owns which system", prompt: "Who owns which system again?" },
    ],
  },
  {
    team: "Finance desk",
    about: "Checks supplier invoices against the payment policy; never pays.",
    tasks: [
      { label: "Approve what we can", prompt: "ok approve whatever we're allowed to and let accounts know" },
      { label: "What goes to the CFO", prompt: "which invoices can we just approve ourselves and which ones have to go to the cfo" },
      { label: "Total owed", prompt: "how much do we owe in total rn? just the number pls" },
    ],
  },
  {
    team: "HR desk",
    about: "Onboards new hires: start dates, managers, welcome emails.",
    tasks: [
      { label: "Welcome mail to Ola", prompt: "can u send ola a welcome mail with the first day checklist" },
      { label: "Who starts in November", prompt: "whos starting in nov and who are their managers" },
    ],
  },
  {
    team: "Research desk",
    about: "Reads web pages and writes short, sourced briefs.",
    tasks: [
      { label: "TLDs in RFC 2606", prompt: "which tlds does rfc 2606 reserve? its at https://www.rfc-editor.org/rfc/rfc2606" },
      { label: "What is example.com", prompt: "what is https://example.com even for?" },
      {
        label: "SQLite: two sources",
        prompt:
          "Compare what https://en.wikipedia.org/wiki/SQLite and https://www.sqlite.org/about.html say about who develops SQLite and how it is licensed. Point out anything they disagree on.",
      },
    ],
  },
]
