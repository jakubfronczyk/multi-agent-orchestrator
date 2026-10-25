import { queryOptions } from "@tanstack/react-query"
import type { AgentId } from "@/generated/AgentId"
import type { AgentInput } from "@/generated/AgentInput"
import type { AgentRecord } from "@/generated/AgentRecord"
import { deleteRequest, getJson, postJson, putJson } from "@/lib/api"

export const agentsQuery = queryOptions({
  queryKey: ["agents"],
  queryFn: () => getJson<AgentRecord[]>("/agents"),
})

export const createAgent = (input: AgentInput) => postJson<AgentRecord>("/agents", input)

export const replaceAgent = (id: AgentId, input: AgentInput) =>
  putJson<AgentRecord>(`/agents/${id}`, input)

export const deleteAgent = (id: AgentId) => deleteRequest(`/agents/${id}`)
