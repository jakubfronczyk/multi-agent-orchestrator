import { queryOptions } from "@tanstack/react-query"
import type { OutboxList } from "@/generated/OutboxList"
import { getJson } from "@/lib/api"

export const outboxQuery = queryOptions({
  queryKey: ["outbox"],
  queryFn: () => getJson<OutboxList>("/outbox"),
})
