import { useEffect, useRef } from "react"
import { useQueryClient, type QueryKey } from "@tanstack/react-query"
import type { EventEnvelope } from "@/generated/EventEnvelope"
import { subscribe } from "./event-hub"

/** Refetches `queryKey` on every event matching `when`; for lists the server derives. */
export function useInvalidateOn(queryKey: QueryKey, when: (event: EventEnvelope) => boolean) {
  const client = useQueryClient()
  const latest = useRef({ queryKey, when })
  useEffect(() => {
    latest.current = { queryKey, when }
  })
  useEffect(
    () =>
      subscribe((event) => {
        if (latest.current.when(event)) {
          void client.invalidateQueries({ queryKey: latest.current.queryKey })
        }
      }),
    [client],
  )
}
