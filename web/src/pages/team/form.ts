import { isApiError } from "@/lib/api"

/** A rejected save: the field the server named, so the message sits under that input. */
export interface Problem {
  field: string | null
  message: string
}

export const problemOf = (error: unknown): Problem =>
  isApiError(error)
    ? { field: error.field, message: error.message }
    : { field: null, message: error instanceof Error ? error.message : "Something went wrong." }

export const inputClass =
  "w-full rounded-md border-0 bg-muted px-3 py-2.5 text-sm text-foreground outline-none ring-1 ring-transparent ring-inset focus:ring-input aria-invalid:ring-destructive"
