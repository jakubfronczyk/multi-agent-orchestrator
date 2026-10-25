import { useSyncExternalStore } from "react"
import type { EventEnvelope } from "@/generated/EventEnvelope"

export type HubStatus = "connecting" | "open" | "reconnecting"

type Listener = (event: EventEnvelope) => void

const BUFFER_LIMIT = 5000
const RETRY_MS = [1000, 2000, 5000]

let source: EventSource | null = null
let status: HubStatus = "connecting"
let lastId = 0
let buffer: EventEnvelope[] = []
let retries = 0
const listeners = new Set<Listener>()
const statusListeners = new Set<() => void>()
const resyncListeners = new Set<() => void>()

function setStatus(next: HubStatus) {
  if (status === next) return
  status = next
  for (const listener of statusListeners) listener()
}

/**
 * Opens the app's one EventSource. While the browser retries on its own it resends
 * `Last-Event-ID` and the server replays the gap. When the stream closes for good (the server
 * was down, so the proxy answered with an error) a new EventSource carries no id, so the gap
 * cannot be replayed: the buffer is dropped and resync listeners refetch their snapshots.
 */
export function startEventHub() {
  if (source) return
  connect(false)
}

function connect(afterLoss: boolean) {
  const next = new EventSource("/api/events")
  source = next
  next.onopen = () => {
    retries = 0
    setStatus("open")
    if (afterLoss) for (const listener of resyncListeners) listener()
  }
  next.onmessage = (message) => {
    const event = JSON.parse(message.data) as EventEnvelope
    if (event.id <= lastId) return
    lastId = event.id
    buffer.push(event)
    if (buffer.length > BUFFER_LIMIT) buffer = buffer.slice(-BUFFER_LIMIT)
    for (const listener of listeners) listener(event)
  }
  next.onerror = () => {
    setStatus("reconnecting")
    if (next.readyState !== EventSource.CLOSED) return
    next.close()
    // Ids restart after a database reset, so the dedupe point is reset with the buffer; the
    // refetched snapshots then decide which events still apply.
    lastId = 0
    buffer = []
    const delay = RETRY_MS[Math.min(retries, RETRY_MS.length - 1)]
    retries += 1
    setTimeout(() => connect(true), delay)
  }
}

/** Calls `listener` for every new event, in id order. */
export function subscribe(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Calls `listener` after a reconnect that could not replay missed events. */
export function onResync(listener: () => void): () => void {
  resyncListeners.add(listener)
  return () => resyncListeners.delete(listener)
}

/** Events received since the hub started with an id above `after`. */
export function bufferedAfter(after: number | null): EventEnvelope[] {
  return after === null ? buffer.slice() : buffer.filter((event) => event.id > after)
}

export function useHubStatus(): HubStatus {
  return useSyncExternalStore(
    (listener) => {
      statusListeners.add(listener)
      return () => statusListeners.delete(listener)
    },
    () => status,
  )
}
