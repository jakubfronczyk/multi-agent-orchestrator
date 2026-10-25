import { useQuery } from "@tanstack/react-query"
import { useSearchParams } from "react-router"
import { agentsQuery } from "@/api/agents"
import { Page } from "@/components/page"
import { AddEditor } from "./add-editor"
import { AgentCards } from "./agent-cards"
import { AgentEditor } from "./agent-editor"

export function TeamPage() {
  const agents = useQuery(agentsQuery)
  const [params, setParams] = useSearchParams()
  const adding = params.get("add") === "1"
  // The Lead is the default selection, so it comes first.
  const list = [...(agents.data ?? [])].sort((a, b) => Number(b.role === "lead") - Number(a.role === "lead"))
  const lead = list.find((agent) => agent.role === "lead")
  const selected = list.find((agent) => agent.slug === params.get("agent")) ?? lead

  return (
    <Page wide title="Your team" lede="Lead hands out the work. Everyone else is a specialist you can tune or add.">
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

      {agents.data && (
        <div className="rounded-lg bg-card ring-1 ring-border">
          {adding ? (
            <AddEditor onDone={(slug) => setParams(slug ? { agent: slug } : {}, { replace: true })} />
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
