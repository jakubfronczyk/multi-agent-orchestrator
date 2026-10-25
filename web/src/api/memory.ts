import { queryOptions } from "@tanstack/react-query"
import type { MemoryList } from "@/generated/MemoryList"
import type { MemoryScope } from "@/generated/MemoryScope"
import type { MemoryVersionList } from "@/generated/MemoryVersionList"
import { getJson, query } from "@/lib/api"

/** Filters match the server's `GET /memory` query parameters. */
export const memoryQuery = (
  filter: { scope?: MemoryScope } & Partial<Record<"agent_id" | "q", string>> = {},
) =>
  queryOptions({
    queryKey: ["memory", filter],
    queryFn: () => getJson<MemoryList>(`/memory${query(filter)}`),
    // No `last_event_id` on this list, so writes made while no screen showed it can't be
    // found in the event buffer; every mount refetches instead.
    refetchOnMount: "always",
  })

export const memoryVersionsQuery = (entryId: string) =>
  queryOptions({
    queryKey: ["memory-versions", entryId],
    queryFn: () => getJson<MemoryVersionList>(`/memory/${entryId}/versions`),
    refetchOnMount: "always",
  })
