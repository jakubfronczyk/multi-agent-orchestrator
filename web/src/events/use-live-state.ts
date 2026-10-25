import { useEffect, useRef, useState } from "react"
import type { EventEnvelope } from "@/generated/EventEnvelope"
import type { EventId } from "@/generated/EventId"
import { bufferedAfter, covers, subscribe } from "./event-hub"

interface Snapshot {
  last_event_id: EventId | null
}

interface Live<S, V> {
  source: S | undefined
  value: V | undefined
  appliedUpTo: number
}

/**
 * A REST snapshot with every later event folded on top: buffered events above its
 * `last_event_id` first, then live ones. Each event applies at most once per snapshot; a new
 * snapshot (a refetch) restarts the fold from it. While the buffer has a hole above the
 * snapshot, it shows unfolded until the hub's resync brings a newer one.
 */
export function useLiveState<S extends Snapshot, V>(
  snapshot: S | undefined,
  init: (snapshot: S) => V,
  reduce: (value: V, event: EventEnvelope) => V,
): V | undefined {
  const fns = useRef({ init, reduce })
  useEffect(() => {
    fns.current = { init, reduce }
  })

  const build = (source: S | undefined): Live<S, V> => {
    if (!source) return { source, value: undefined, appliedUpTo: 0 }
    const events = covers(source.last_event_id) ? bufferedAfter(source.last_event_id) : []
    return {
      source,
      value: events.reduce(reduce, init(source)),
      appliedUpTo: events.at(-1)?.id ?? source.last_event_id ?? 0,
    }
  }

  const [live, setLive] = useState(() => build(snapshot))
  // A new snapshot rebuilds during render, so no frame shows the old fold with the new data.
  const current = live.source === snapshot ? live : build(snapshot)
  if (current !== live) setLive(current)

  useEffect(() => {
    if (!snapshot) return
    const apply = (events: EventEnvelope[]) =>
      setLive((current) => {
        if (current.source !== snapshot || current.value === undefined) return current
        if (!covers(current.appliedUpTo)) return current
        const fresh = events.filter((event) => event.id > current.appliedUpTo)
        if (fresh.length === 0) return current
        return {
          source: current.source,
          value: fresh.reduce(fns.current.reduce, current.value),
          appliedUpTo: fresh.at(-1)!.id,
        }
      })
    // Reads the buffer each time, so events that arrived between the render that built the
    // state and this subscription, or before the buffer covered it, are not skipped.
    const catchUp = () => apply(bufferedAfter(snapshot.last_event_id))
    catchUp()
    return subscribe(catchUp)
  }, [snapshot])

  return current.value
}
