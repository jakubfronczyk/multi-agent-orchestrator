import { queryOptions } from "@tanstack/react-query"
import type { ToolCallList } from "@/generated/ToolCallList"
import { getJson, query } from "@/lib/api"

/** Filters match the server's `GET /tool-calls` query parameters. */
export const toolCallsQuery = (
  filter: Partial<Record<"run_id" | "agent_id" | "tool" | "status", string>> & { limit?: number } = {},
) =>
  queryOptions({
    queryKey: ["tool-calls", filter],
    queryFn: () => getJson<ToolCallList>(`/tool-calls${query(filter)}`),
  })
