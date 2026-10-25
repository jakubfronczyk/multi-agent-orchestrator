import { queryOptions } from "@tanstack/react-query"
import type { Health } from "@/generated/Health"
import type { ModelList } from "@/generated/ModelList"
import type { ToolList } from "@/generated/ToolList"
import { getJson } from "@/lib/api"

// Server configuration does not change while it runs, so these are fetched once per page load.
export const healthQuery = queryOptions({
  queryKey: ["health"],
  queryFn: () => getJson<Health>("/health"),
  staleTime: Infinity,
})

export const toolsQuery = queryOptions({
  queryKey: ["tools"],
  queryFn: () => getJson<ToolList>("/tools"),
  staleTime: Infinity,
})

export const modelsQuery = queryOptions({
  queryKey: ["models"],
  queryFn: () => getJson<ModelList>("/models"),
  staleTime: Infinity,
})
