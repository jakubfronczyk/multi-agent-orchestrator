import { useEffect, useRef } from "react"
import { useQueryClient, type QueryKey } from "@tanstack/react-query"
import type { EventEnvelope } from "@/generated/EventEnvelope"
import { bufferedAfter, subscribe } from "./event-hub"
import { refetchNow } from "./refetch"

function lastEventId(data: unknown): number | null | undefined {
  if (typeof data !== "object" || data === null || !("last_event_id" in data)) return undefined
  return data.last_event_id as number | null
}

/**
 * Refetches `queryKey` on every event matching `when`; for lists the server derives. On mount it
 * also checks the buffer: a matching event that arrived while no screen listened would otherwise
 * leave the cached list stale, since queries never go stale by age. With `delayMs`, a burst of
 * matching events within that window causes one refetch.
 */
export function useInvalidateOn(
  queryKey: QueryKey,
  when: (event: EventEnvelope) => boolean,
  delayMs = 0,
) {
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
    if (missed) void refetchNow(client, queryKey)
    let timer: ReturnType<typeof setTimeout> | undefined
    const refetch = () => {
      timer = undefined
      void refetchNow(client, latest.current.queryKey)
    }
    const unsubscribe = subscribe((event) => {
      if (!latest.current.when(event)) return
      if (delayMs === 0) refetch()
      else timer ??= setTimeout(refetch, delayMs)
    })
    return () => {
      unsubscribe()
      clearTimeout(timer)
    }
  }, [client, delayMs])
}
