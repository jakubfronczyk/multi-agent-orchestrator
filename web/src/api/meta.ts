import { queryOptions } from "@tanstack/react-query"
import type { Health } from "@/generated/Health"
import type { ModelList } from "@/generated/ModelList"
import type { ToolList } from "@/generated/ToolList"
import { getJson } from "@/lib/api"

export const healthQuery = queryOptions({
  queryKey: ["health"],
  queryFn: () => getJson<Health>("/health"),
})

export const toolsQuery = queryOptions({
  queryKey: ["tools"],
  queryFn: () => getJson<ToolList>("/tools"),
})

export const modelsQuery = queryOptions({
  queryKey: ["models"],
  queryFn: () => getJson<ModelList>("/models"),
})
