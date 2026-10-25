import { useSyncExternalStore } from "react"

export type ThemeChoice = "light" | "dark" | "system"

const KEY = "theme"
const dark = window.matchMedia("(prefers-color-scheme: dark)")
const listeners = new Set<() => void>()

function read(): ThemeChoice {
  try {
    const stored = localStorage.getItem(KEY)
    return stored === "light" || stored === "dark" ? stored : "system"
  } catch {
    return "system"
  }
}

let choice = read()

function apply() {
  const isDark = choice === "dark" || (choice === "system" && dark.matches)
  document.documentElement.classList.toggle("dark", isDark)
}

/** Keeps `.dark` on <html> in step with the choice and, for System, with the OS setting. */
export function startTheme() {
  apply()
  dark.addEventListener("change", apply)
}

export function setTheme(next: ThemeChoice) {
  choice = next
  try {
    if (next === "system") localStorage.removeItem(KEY)
    else localStorage.setItem(KEY, next)
  } catch {
    // Storage can be blocked; the choice then lasts for this page load only.
  }
  apply()
  for (const listener of listeners) listener()
}

export function useTheme(): ThemeChoice {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => choice,
  )
}
