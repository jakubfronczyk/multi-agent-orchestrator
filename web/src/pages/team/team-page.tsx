import { useRef, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useSearchParams } from "react-router"
import { agentsQuery } from "@/api/agents"
import { runsQuery } from "@/api/runs"
import { deleteTeam, refetchTeams, renameTeam } from "@/api/teams"
import { Page } from "@/components/page"
import { Button } from "@/components/ui/button"
import type { TeamView } from "@/generated/TeamView"
import { isApiError } from "@/lib/api"
import { useSelectedTeam } from "@/lib/selected-team"
import { AddEditor } from "./add-editor"
import { AgentCards } from "./agent-cards"
import { AgentEditor } from "./agent-editor"

export function TeamPage() {
  const agents = useQuery(agentsQuery)
  const { team } = useSelectedTeam()
  const [params, setParams] = useSearchParams()
  const adding = params.get("add") === "1"
  // Only the selected team's agents; the Lead is the default selection, so it comes first.
  const list = (agents.data ?? [])
    .filter((agent) => agent.team_id === team?.id)
    .sort((a, b) => Number(b.role === "lead") - Number(a.role === "lead"))
  const lead = list.find((agent) => agent.role === "lead")
  const selected = list.find((agent) => agent.slug === params.get("agent")) ?? lead

  return (
    <Page wide title="Your team" lede="The Lead hands out the work. Everyone else is a specialist you can tune or add.">
      {team && <TeamHeader key={team.id} team={team} />}
      {agents.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load the team. {agents.error.message}</p>
      ) : (
        <AgentCards
          agents={list}
          selectedId={selected?.id}
          adding={adding}
          onSelect={(agent) => setParams({ agent: agent.slug }, { replace: true })}
          onAdd={() => setParams({ add: "1" }, { replace: true })}
        />
      )}
      {agents.data && list.length === 1 && !adding && (
        <p className="m-0 text-sm text-muted-foreground">Add a teammate so the Lead has someone to hand work to.</p>
      )}

      {agents.data && team && (
        <div className="rounded-lg bg-card ring-1 ring-border">
          {adding ? (
            <AddEditor teamId={team.id} onDone={(slug) => setParams(slug ? { agent: slug } : {}, { replace: true })} />
          ) : selected ? (
            <AgentEditor
              key={selected.id}
              agent={selected}
              team={list}
              onRemoved={() => setParams({}, { replace: true })}
            />
          ) : null}
        </div>
      )}
    </Page>
  )
}

/** The team's name, with rename and delete. Team writes emit no events, so the lists refetch here. */
function TeamHeader({ team }: { team: TeamView }) {
  const client = useQueryClient()
  const [mode, setMode] = useState<"view" | "rename" | "delete">("view")
  const [name, setName] = useState(team.name)
  const [error, setError] = useState<string | null>(null)
  // The live task that blocked a delete, linked from the 409 notice.
  const [blocker, setBlocker] = useState<string | null>(null)
  const clearError = () => {
    setError(null)
    setBlocker(null)
  }
  const renameButton = useRef<HTMLButtonElement>(null)
  // The form unmounts on save or cancel; focus goes back to the button that opened it.
  const closeForm = () => {
    setMode("view")
    requestAnimationFrame(() => renameButton.current?.focus())
  }
  const refresh = () => refetchTeams(client)
  const rename = useMutation({
    mutationFn: () => renameTeam(team.id, { name }),
    onSuccess: async () => {
      await refresh()
      closeForm()
    },
    onError: (failure) => setError(isApiError(failure, 422) ? failure.message : `Couldn't rename it. ${failure.message}`),
  })
  const remove = useMutation({
    mutationFn: () => deleteTeam(team.id),
    // The switcher falls back to the oldest live team once the list no longer has this one.
    onSuccess: refresh,
    onError: async (failure) => {
      if (isApiError(failure, 409) && failure.message.includes("running")) {
        setError("Cancel its running task first.")
        const { runs } = await client.fetchQuery({ ...runsQuery(50, team.id), staleTime: 0 })
        setBlocker(runs.find((run) => run.status === "running")?.id ?? null)
      } else {
        setError(isApiError(failure, 409) ? "Keep at least one team." : `Couldn't delete it. ${failure.message}`)
      }
    },
  })

  return (
    <section aria-label="Team" className="flex flex-col gap-3">
      {mode === "rename" ? (
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            if (name.trim() && !rename.isPending) rename.mutate()
          }}
        >
          <input
            autoFocus
            aria-label="Team name"
            value={name}
            maxLength={60}
            onChange={(event) => {
              setName(event.target.value)
              clearError()
            }}
            className="h-9 min-w-[200px] flex-1 rounded-md bg-card px-3 ring-1 ring-border outline-none ring-inset focus-visible:ring-2 focus-visible:ring-ring"
          />
          <Button type="submit" size="sm" disabled={!name.trim() || rename.isPending}>
            Save name
          </Button>
          <Button type="button" size="sm" variant="ghost" onClick={closeForm}>
            Cancel
          </Button>
        </form>
      ) : (
        <div className="flex flex-wrap items-center gap-3">
          <h2 className="m-0 min-w-0 text-xl font-semibold tracking-tight break-words">{team.name}</h2>
          <span className="text-sm text-muted-foreground">
            {team.agent_count} {team.agent_count === 1 ? "agent" : "agents"}
          </span>
          <span className="ml-auto flex gap-1">
            <Button
              ref={renameButton}
              size="sm"
              variant="ghost"
              onClick={() => {
                clearError()
                setName(team.name)
                setMode("rename")
              }}
            >
              Rename
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="text-destructive"
              onClick={() => {
                clearError()
                setMode("delete")
              }}
            >
              Delete team
            </Button>
          </span>
        </div>
      )}
      {mode === "delete" && (
        <div className="flex flex-wrap items-center gap-3 rounded-md bg-muted px-4 py-3 text-sm">
          <span className="min-w-[200px] flex-1">
            Delete {team.name}? Tasks, notes and files stay; its agents are removed.
          </span>
          <Button size="sm" disabled={remove.isPending} onClick={() => remove.mutate()}>
            Delete
          </Button>
          <Button size="sm" variant="outline" autoFocus onClick={() => setMode("view")}>
            Keep
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="m-0 text-sm text-destructive">
          {error}
          {blocker && (
            <>
              {" "}
              <Link to={`/tasks/${blocker}`} className="underline underline-offset-[3px]">
                Open the running task
              </Link>
            </>
          )}
        </p>
      )}
    </section>
  )
}
