import type { ReactNode } from "react"
import { useQuery } from "@tanstack/react-query"
import { cn } from "cn"
import { modelsQuery } from "@/api/meta"
import type { AgentInput } from "@/generated/AgentInput"
import type { Model } from "@/generated/Model"
import type { ReasoningEffort } from "@/generated/ReasoningEffort"
import { EFFORT_LABEL, MODEL_LABEL } from "@/lib/team"
import { inputClass, type Problem } from "./form"

export function Field({
  label,
  htmlFor,
  error,
  children,
}: {
  label: string
  htmlFor?: string
  error?: string | null
  children: ReactNode
}) {
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {htmlFor ? (
        <label htmlFor={htmlFor} className="text-sm font-medium text-sidebar-foreground">
          {label}
        </label>
      ) : (
        <span className="text-sm font-medium text-sidebar-foreground">{label}</span>
      )}
      {children}
      {error && (
        <p role="alert" className="m-0 text-sm text-destructive">
          {error}
        </p>
      )}
    </div>
  )
}

/** Description, instructions, model, effort and the fixed step limit. */
export function CommonFields({
  draft,
  setDraft,
  maxSteps,
  problem,
  idPrefix,
}: {
  draft: AgentInput
  setDraft: (next: AgentInput) => void
  maxSteps: number | null
  problem: Problem | null
  idPrefix: string
}) {
  const models = useQuery(modelsQuery)
  const efforts = models.data?.models.find((m) => m.id === draft.model)?.efforts ?? []
  const fieldError = (field: string) => (problem?.field === field ? problem.message : null)
  return (
    <>
      <Field label="Description" htmlFor={`${idPrefix}-desc`} error={fieldError("description")}>
        <textarea
          id={`${idPrefix}-desc`}
          value={draft.description}
          aria-invalid={problem?.field === "description"}
          placeholder="What this teammate is for, and what it is not for. Lead reads this to pick who gets a step."
          onChange={(event) => setDraft({ ...draft, description: event.target.value })}
          className={cn(inputClass, "min-h-[76px] resize-y leading-normal")}
        />
      </Field>
      <Field label="Instructions" htmlFor={`${idPrefix}-ins`} error={fieldError("system_prompt")}>
        <textarea
          id={`${idPrefix}-ins`}
          value={draft.system_prompt}
          aria-invalid={problem?.field === "system_prompt"}
          placeholder="Explain the job the way you'd brief a new colleague."
          onChange={(event) => setDraft({ ...draft, system_prompt: event.target.value })}
          className={cn(inputClass, "min-h-[76px] resize-y leading-normal")}
        />
      </Field>
      <div className="flex flex-col gap-2">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
          <Field label="Model" htmlFor={`${idPrefix}-model`} error={fieldError("model")}>
            <select
              id={`${idPrefix}-model`}
              value={draft.model}
              onChange={(event) => {
                const model = event.target.value as Model
                const allowed = models.data?.models.find((m) => m.id === model)?.efforts ?? []
                // A model without efforts takes none; switching keeps the effort only if it still fits.
                const effort =
                  allowed.length === 0
                    ? undefined
                    : draft.reasoning_effort && allowed.includes(draft.reasoning_effort)
                      ? draft.reasoning_effort
                      : allowed[0]
                setDraft({ ...draft, model, reasoning_effort: effort })
              }}
              className={inputClass}
            >
              {(models.data?.models ?? []).map((m) => (
                <option key={m.id} value={m.id}>
                  {MODEL_LABEL[m.id]}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Effort" htmlFor={`${idPrefix}-effort`} error={fieldError("reasoning_effort")}>
            <select
              id={`${idPrefix}-effort`}
              value={draft.reasoning_effort ?? ""}
              disabled={efforts.length === 0}
              onChange={(event) => setDraft({ ...draft, reasoning_effort: event.target.value as ReasoningEffort })}
              className={cn(inputClass, "disabled:opacity-60")}
            >
              {efforts.length === 0 ? (
                <option value="">Not used</option>
              ) : (
                efforts.map((effort) => (
                  <option key={effort} value={effort}>
                    {EFFORT_LABEL[effort]}
                  </option>
                ))
              )}
            </select>
          </Field>
          <Field label="Max steps">
            <span className="px-1 py-2.5 text-sm text-foreground">{maxSteps ?? "Set by the app"}</span>
          </Field>
        </div>
        <p className="m-0 text-sm text-muted-foreground">
          {efforts.length > 0
            ? "Higher effort thinks longer and costs more."
            : "This model answers without thinking first, so effort isn't used."}{" "}
          Max steps is how many model steps it gets before it must answer.
        </p>
      </div>
    </>
  )
}
