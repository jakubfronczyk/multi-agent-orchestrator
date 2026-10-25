import { queryOptions } from "@tanstack/react-query"
import type { OutboxList } from "@/generated/OutboxList"
import { getJson, query } from "@/lib/api"

/** Sent messages, of one team when `teamId` is set; every key starts with `["outbox"]`. */
export const outboxQuery = (teamId?: string) =>
  queryOptions({
    queryKey: ["outbox", teamId ? { team_id: teamId } : {}],
    queryFn: () => getJson<OutboxList>(`/outbox${query({ team_id: teamId })}`),
  })
