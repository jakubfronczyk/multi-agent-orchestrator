import { useEffect, useRef } from "react"
import { useQueryClient, type QueryKey } from "@tanstack/react-query"
import type { EventEnvelope } from "@/generated/EventEnvelope"
import { bufferedAfter, subscribe } from "./event-hub"

function lastEventId(data: unknown): number | null | undefined {
  if (typeof data !== "object" || data === null || !("last_event_id" in data)) return undefined
  return data.last_event_id as number | null
}

/**
 * Refetches `queryKey` on every event matching `when`; for lists the server derives. On mount it
 * also checks the buffer: a matching event that arrived while no screen listened would otherwise
 * leave the cached list stale, since queries never go stale by age.
 */
export function useInvalidateOn(queryKey: QueryKey, when: (event: EventEnvelope) => boolean) {
  const client = useQueryClient()
  const latest = useRef({ queryKey, when })
  useEffect(() => {
    latest.current = { queryKey, when }
  })
  useEffect(() => {
    const { queryKey, when } = latest.current
    const missed = client.getQueriesData({ queryKey }).some(([, data]) => {
      const after = lastEventId(data)
      return after !== undefined && bufferedAfter(after).some(when)
    })
    if (missed) void client.invalidateQueries({ queryKey })
    return subscribe((event) => {
      if (latest.current.when(event)) {
        void client.invalidateQueries({ queryKey: latest.current.queryKey })
      }
    })
  }, [client])
}
