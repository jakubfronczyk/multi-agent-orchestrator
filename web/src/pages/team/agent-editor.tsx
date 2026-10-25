import { useRef, useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { agentsQuery, deleteAgent, replaceAgent } from "@/api/agents"
import { AgentShape } from "@/components/agent-shape"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import type { AgentInput } from "@/generated/AgentInput"
import type { AgentRecord } from "@/generated/AgentRecord"
import { inputOf } from "@/lib/team"
import { CommonFields, Field } from "./fields"
import { inputClass, problemOf, type Problem } from "./form"
import { ToolRows } from "./tool-rows"

export function AgentEditor({ agent, team, onRemoved }: { agent: AgentRecord; team: AgentRecord[]; onRemoved: () => void }) {
  const client = useQueryClient()
  const [draft, setDraft] = useState<AgentInput>(() => inputOf(agent))
  const [problem, setProblem] = useState<Problem | null>(null)
  const [saved, setSaved] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const removeButton = useRef<HTMLButtonElement>(null)
  const isLead = agent.role === "lead"
  const specialists = team.filter((member) => member.role === "specialist")

  const save = useMutation({
    mutationFn: () => replaceAgent(agent.id, draft),
    onSuccess: () => {
      setSaved(true)
      void client.invalidateQueries({ queryKey: agentsQuery.queryKey })
    },
    onError: (error) => setProblem(problemOf(error)),
  })
  const remove = useMutation({
    mutationFn: () => deleteAgent(agent.id),
    onSuccess: async () => {
      // The Lead's targets change on the server too; the refetch shows both.
      await client.invalidateQueries({ queryKey: agentsQuery.queryKey })
      onRemoved()
    },
    onError: (error) => setProblem(problemOf(error)),
  })

  const update = (next: AgentInput) => {
    setDraft(next)
    setSaved(false)
    setProblem(null)
  }

  return (
    <div className="flex animate-in flex-col gap-6 p-6 duration-300 fade-in">
      <div className="flex items-center gap-3">
        <AgentShape icon={agent.icon} colour={agent.colour} size="lg" />
        <div>
          <b className="block text-lg font-semibold">{agent.name}</b>
          <small className="text-sm text-muted-foreground">
            {isLead ? "Every team has one Lead. It can't be removed." : `Lead sees it as ${agent.slug}`}
          </small>
        </div>
      </div>

      <Field label="Name" htmlFor="edit-name" error={problem?.field === "name" ? problem.message : null}>
        <input
          id="edit-name"
          value={draft.name}
          aria-invalid={problem?.field === "name"}
          onChange={(event) => update({ ...draft, name: event.target.value })}
          className={inputClass}
        />
      </Field>
      <CommonFields draft={draft} setDraft={update} maxSteps={agent.max_steps} problem={problem} idPrefix="edit" />

      {isLead && (
        <Field label="Who Lead can hand work to" error={problem?.field === "delegate_targets" ? problem.message : null}>
          <div className="flex flex-col gap-2">
            {specialists.map((member) => {
              const on = draft.delegate_targets.includes(member.slug)
              return (
                <label key={member.id} className="flex items-center gap-3 text-sm">
                  <Switch
                    checked={on}
                    onCheckedChange={(checked) =>
                      update({
                        ...draft,
                        delegate_targets: checked
                          ? [...draft.delegate_targets, member.slug]
                          : draft.delegate_targets.filter((slug) => slug !== member.slug),
                      })
                    }
                  />
                  <AgentShape icon={member.icon} colour={member.colour} />
                  {member.name}
                </label>
              )
            })}
          </div>
        </Field>
      )}

      <ToolRows role={agent.role} draft={draft} setDraft={update} problem={problem} />

      <p className="m-0 rounded-md bg-muted px-4 py-3 text-sm text-sidebar-foreground">
        {isLead
          ? "Lead only plans and hands out work. It can read trusted shared notes but never writes them, and it can't have file, inbox, website or message tools."
          : "If a task reads something from outside, like an email in your inbox, file saves and website fetches set to Allow ask you first. Notes it writes are kept untrusted."}
      </p>

      {problem && !["name", "description", "system_prompt", "model", "reasoning_effort", "tools", "delegate_targets"].includes(problem.field ?? "") && (
        <p role="alert" className="m-0 text-sm text-destructive">
          {problem.message}
        </p>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button disabled={save.isPending} onClick={() => save.mutate()}>
          Save changes
        </Button>
        {saved && (
          <span role="status" className="text-sm text-emerald-700 dark:text-emerald-400">
            Saved. {draft.name} uses this from the next task.
          </span>
        )}
        {!isLead && !confirming && (
          <button
            ref={removeButton}
            type="button"
            onClick={() => setConfirming(true)}
            className="ml-auto text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"
          >
            Remove from team
          </button>
        )}
      </div>
      {confirming && (
        <div className="flex flex-wrap items-center gap-3 rounded-md bg-muted px-4 py-3 text-sm">
          <span className="min-w-[200px] flex-1">
            Remove {agent.name}? Lead won't hand it work any more. The activity log keeps what it did.
          </span>
          <Button size="sm" disabled={remove.isPending} onClick={() => remove.mutate()}>
            Remove
          </Button>
          <Button
            size="sm"
            variant="outline"
            autoFocus
            onClick={() => {
              setConfirming(false)
              // The button remounts with the confirm gone; focus it once it is back.
              requestAnimationFrame(() => removeButton.current?.focus())
            }}
          >
            Keep
          </Button>
        </div>
      )}
    </div>
  )
}
