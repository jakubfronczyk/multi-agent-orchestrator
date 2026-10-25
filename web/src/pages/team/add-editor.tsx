import { useState } from "react"
import { useMutation, useQueryClient } from "@tanstack/react-query"
import { cn } from "cn"
import { agentsQuery, createAgent, replaceAgent } from "@/api/agents"
import { AgentShape } from "@/components/agent-shape"
import { Button } from "@/components/ui/button"
import { Switch } from "@/components/ui/switch"
import type { AgentInput } from "@/generated/AgentInput"
import type { AgentRecord } from "@/generated/AgentRecord"
import { inputOf, PRESETS, presetInput, slugFrom, type Preset } from "@/lib/team"
import { CommonFields, Field } from "./fields"
import { inputClass, problemOf, type Problem } from "./form"
import { ToolRows } from "./tool-rows"

export function AddEditor({ teamId, onDone }: { teamId: string; onDone: (slug: string | null) => void }) {
  const client = useQueryClient()
  const [preset, setPreset] = useState<Preset | null>(null)
  const [draft, setDraft] = useState<AgentInput | null>(null)
  const [slugEdited, setSlugEdited] = useState(false)
  const [reachable, setReachable] = useState(true)
  const [problem, setProblem] = useState<Problem | null>(null)
  const [warning, setWarning] = useState<string | null>(null)
  // Set once the agent exists but the Lead update failed; a retry then only updates the Lead.
  const [joined, setJoined] = useState<AgentRecord | null>(null)

  const linkLead = async (created: AgentRecord): Promise<string | null> => {
    try {
      // A fresh read: a Lead rebuilt from the cache would overwrite an edit saved since.
      const agents = await client.fetchQuery({ ...agentsQuery, staleTime: 0 })
      const lead = agents.find((agent) => agent.team_id === teamId && agent.role === "lead")
      if (!lead) throw new Error("the team has no Lead")
      if (!lead.delegate_targets.includes(created.slug)) {
        await replaceAgent(lead.id, { ...inputOf(lead), delegate_targets: [...lead.delegate_targets, created.slug] })
      }
      return null
    } catch (error) {
      return problemOf(error).message
    }
  }

  const add = useMutation({
    mutationFn: async (input: AgentInput) => {
      const created = joined ?? (await createAgent({ ...input, team_id: teamId }))
      return { created, leadError: reachable || joined ? await linkLead(created) : null }
    },
    onSuccess: async ({ created, leadError }) => {
      await client.invalidateQueries({ queryKey: agentsQuery.queryKey })
      // A team's agent count changes with it; team writes and agent writes emit no events.
      await client.invalidateQueries({ queryKey: ["teams"] })
      if (leadError) {
        setJoined(created)
        setWarning(`${created.name} joined the team, but Lead can't hand it work yet: ${leadError}`)
      } else {
        onDone(created.slug)
      }
    },
    onError: (error) => setProblem(problemOf(error)),
  })

  const choose = (next: Preset) => {
    const name = draft?.name ?? ""
    setPreset(next)
    setDraft({ ...presetInput(next, name), slug: slugEdited && draft ? draft.slug : slugFrom(name) })
    setProblem(null)
  }
  const update = (next: AgentInput) => {
    setDraft(next)
    setProblem(null)
  }

  return (
    <div className="flex animate-in flex-col gap-6 p-6 duration-300 fade-in">
      <div className="flex items-center gap-3">
        <AgentShape icon={preset?.icon ?? "hexagon"} colour={preset?.colour} size="lg" />
        <div>
          <b className="block text-lg font-semibold">Add a teammate</b>
          <small className="text-sm text-muted-foreground">Lead decides when to hand it work</small>
        </div>
        <Button variant="ghost" className="ml-auto" onClick={() => onDone(null)}>
          Cancel
        </Button>
      </div>

      <Field label="Start from">
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-4">
          {PRESETS.map((p) => (
            <button
              key={p.id}
              type="button"
              aria-pressed={preset?.id === p.id}
              onClick={() => choose(p)}
              className={cn(
                "flex flex-col gap-1 rounded-md p-3 text-left ring-1 ring-border ring-inset hover:ring-input",
                preset?.id === p.id && "ring-[1.5px] ring-foreground hover:ring-foreground",
              )}
            >
              <AgentShape icon={p.icon} colour={p.colour} size="md" />
              <b className="text-sm font-medium">{p.name}</b>
              <small className="text-xs/snug text-muted-foreground">{p.detail}</small>
            </button>
          ))}
        </div>
      </Field>

      {draft && (
        <>
          <Field label="Name" htmlFor="add-name" error={problem?.field === "name" ? problem.message : null}>
            <input
              id="add-name"
              value={draft.name}
              aria-invalid={problem?.field === "name"}
              placeholder="For example, Contracts reader"
              onChange={(event) =>
                update({
                  ...draft,
                  name: event.target.value,
                  slug: slugEdited ? draft.slug : slugFrom(event.target.value),
                })
              }
              className={inputClass}
            />
          </Field>
          <Field label="Short name" htmlFor="add-slug" error={problem?.field === "slug" ? problem.message : null}>
            <input
              id="add-slug"
              value={draft.slug}
              aria-invalid={problem?.field === "slug"}
              onChange={(event) => {
                setSlugEdited(true)
                update({ ...draft, slug: event.target.value })
              }}
              className={cn(inputClass, "font-mono")}
            />
            <p className="m-0 text-sm text-muted-foreground">
              Lead sees it as {draft.slug || "…"}. Lowercase letters, digits, dash and underscore; it can't change later.
            </p>
          </Field>
          <CommonFields draft={draft} setDraft={update} maxSteps={null} problem={problem} idPrefix="add" />
          <ToolRows role="specialist" draft={draft} setDraft={update} problem={problem} />
          <label className="flex items-center gap-3 text-sm">
            <Switch checked={reachable} onCheckedChange={setReachable} />
            Lead can hand it work
          </label>
          <p className="m-0 rounded-md bg-muted px-4 py-3 text-sm text-sidebar-foreground">
            Tools come from one fixed list. Off means this teammate doesn't have the tool at all. You can't add new tools.
          </p>
          {problem && !["name", "slug", "description", "system_prompt", "model", "reasoning_effort", "tools"].includes(problem.field ?? "") && (
            <p role="alert" className="m-0 text-sm text-destructive">
              {problem.message}
            </p>
          )}
          {warning && (
            <p role="status" className="m-0 text-sm text-amber-800 dark:text-amber-300">
              {warning}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <Button disabled={add.isPending} onClick={() => add.mutate(draft)}>
              {joined ? "Let Lead hand it work" : "Add to team"}
            </Button>
          </div>
        </>
      )}
    </div>
  )
}
