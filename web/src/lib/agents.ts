import { useQuery } from "@tanstack/react-query"
import { agentsQuery } from "@/api/agents"
import type { AgentRecord } from "@/generated/AgentRecord"

/** Agent lookups by id and by slug from the app-wide agents cache. */
export function useAgents() {
  const { data = [] } = useQuery(agentsQuery)
  const byId = (id: string): AgentRecord | undefined => data.find((agent) => agent.id === id)
  const bySlug = (slug: string): AgentRecord | undefined => data.find((agent) => agent.slug === slug)
  // A slug the team no longer has is shown as sent.
  const nameOfSlug = (slug: string) => bySlug(slug)?.name ?? slug
  return { agents: data, byId, bySlug, nameOfSlug }
}
