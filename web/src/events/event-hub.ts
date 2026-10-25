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
// Every event above `floor` is in the buffer; at or below it some may be missing. Infinity after
// a hard close, until the first event of the new stream shows where it starts.
let floor = 0
// The tool of every call requested since the app started; a finish names no tool when it is ok.
const callTools = new Map<string, string>()
// Delegated runs seen since the app started; their ends change no task list.
const childRuns = new Set<string>()
const RECORD_LIMIT = 5000
let retries = 0
// Set on every reopen; the first event after it tells a database reset from a replay.
let checkForReset = false
let opened = false
const listeners = new Set<Listener>()
const statusListeners = new Set<() => void>()
const resyncListeners = new Set<() => void>()

function setStatus(next: HubStatus) {
  if (status === next) return
  status = next
  for (const listener of statusListeners) listener()
}

function resync() {
  for (const listener of resyncListeners) listener()
}

/**
 * Opens the app's one EventSource. Every open, the first included, resyncs: the stream starts
 * live-only, so a snapshot fetched before it opened may miss an event committed in between.
 * While the browser retries on its own it resends `Last-Event-ID` and the server replays the
 * gap. When the stream closes for good (the server was down, so the proxy answered with an
 * error) a new EventSource carries no id, so the buffer is dropped and snapshots are refetched.
 */
export function startEventHub() {
  if (source) return
  connect()
}

function connect() {
  const next = new EventSource("/api/events")
  source = next
  next.onopen = () => {
    retries = 0
    checkForReset = opened
    opened = true
    // The resync below refetches every snapshot, so older events are already in them; kept,
    // they would fold onto a post-reset snapshot as ghosts. A soft reconnect replays everything
    // above `lastId`; after a hard close the start is unknown.
    buffer = []
    if (floor !== Infinity) floor = lastId
    setStatus("open")
    resync()
  }
  next.onmessage = (message) => {
    const event = JSON.parse(message.data) as EventEnvelope
    const reset = checkForReset && event.id <= lastId
    checkForReset = false
    // A replay only sends ids above `Last-Event-ID`, so a lower first id means the database
    // was reset and ids restarted; anything else at or below `lastId` is a duplicate.
    if (reset) {
      buffer = []
      floor = event.id - 1
    } else if (event.id <= lastId) {
      return
    }
    if (floor === Infinity) floor = event.id - 1
    lastId = event.id
    if (event.type === "tool_call_requested") {
      callTools.set(event.data.call_id, event.data.tool)
      if (callTools.size > RECORD_LIMIT) callTools.delete(callTools.keys().next().value!)
    }
    if (event.type === "run_started" && event.data.parent_run_id !== null) {
      childRuns.add(event.run_id)
      if (childRuns.size > RECORD_LIMIT) childRuns.delete(childRuns.values().next().value!)
    }
    buffer.push(event)
    // Trimming opens a hole below the kept events, so snapshots refetch past it.
    const trimmed = buffer.length > BUFFER_LIMIT
    if (trimmed) {
      floor = buffer[buffer.length - BUFFER_LIMIT - 1].id
      buffer = buffer.slice(-BUFFER_LIMIT)
    }
    for (const listener of listeners) listener(event)
    if (reset || trimmed) resync()
  }
  next.onerror = () => {
    setStatus("reconnecting")
    if (next.readyState !== EventSource.CLOSED) return
    next.close()
    // Ids restart after a database reset, so the dedupe point is reset with the buffer; the
    // refetched snapshots then decide which events still apply.
    lastId = 0
    buffer = []
    floor = Infinity
    const delay = RETRY_MS[Math.min(retries, RETRY_MS.length - 1)]
    retries += 1
    setTimeout(connect, delay)
  }
}

/** Calls `listener` for every new event, in id order. */
export function subscribe(listener: Listener): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Calls `listener` on every open and after a database reset; snapshots refetch there. */
export function onResync(listener: () => void): () => void {
  resyncListeners.add(listener)
  return () => resyncListeners.delete(listener)
}

/** Events received since the hub started with an id above `after`. */
export function bufferedAfter(after: number | null): EventEnvelope[] {
  return after === null ? buffer.slice() : buffer.filter((event) => event.id > after)
}

/** Whether the buffer holds every event above `after`, so a snapshot at `after` folds without a hole. */
export function covers(after: number | null): boolean {
  return (after ?? 0) >= floor
}

/** The tool a call was requested for, when its request arrived since the app started. */
export function toolOf(callId: string): string | undefined {
  return callTools.get(callId)
}

/** Whether `runId` is a delegated run whose start arrived since the app started. */
export function isChildRun(runId: string): boolean {
  return childRuns.has(runId)
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
