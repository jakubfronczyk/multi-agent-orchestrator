import { useQuery } from "@tanstack/react-query"
import { agentsQuery } from "@/api/agents"
import type { AgentRecord } from "@/generated/AgentRecord"

/**
 * Agent lookups from the app-wide agents cache; slugs are unique only within a team, so they
 * resolve in `teamId`, the team of the record shown. Without one a slug resolves to nothing.
 */
export function useAgents(teamId?: string) {
  const { data = [] } = useQuery(agentsQuery)
  const byId = (id: string): AgentRecord | undefined => data.find((agent) => agent.id === id)
  const bySlug = (slug: string): AgentRecord | undefined =>
    teamId === undefined ? undefined : data.find((agent) => agent.team_id === teamId && agent.slug === slug)
  // A slug the team no longer has is shown as sent.
  // An agent of a deleted team is not in the live list; its slug, capitalised, reads as a name.
  const nameOfSlug = (slug: string) => bySlug(slug)?.name ?? slug.charAt(0).toUpperCase() + slug.slice(1)
  return { agents: data, byId, bySlug, nameOfSlug }
}
