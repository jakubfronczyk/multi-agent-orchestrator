import type { EventEnvelope } from "../src/generated/EventEnvelope.ts"
import { BASE } from "./api.ts"
import { message, sleep } from "./util.ts"

/**
 * Records `/api/events` for the whole eval. A dropped stream reconnects with `Last-Event-ID`, so
 * the server replays what was missed; an event is kept once even if a replay repeats it.
 */
export class EventRecorder {
  readonly events: EventEnvelope[] = []
  /** Ids seen more than once in the raw stream, before dedup. */
  readonly duplicates: number[] = []
  readonly reconnects: string[] = []
  private seen = new Set<number>()
  private lastId: number | null = null
  private controller = new AbortController()
  private opened: Promise<void>
  private markOpen: () => void = () => undefined

  /** With `after`, the server first replays every event after that id. */
  constructor(after: number | null = null) {
    this.lastId = after
    this.opened = new Promise((resolve) => (this.markOpen = resolve))
    void this.loop()
  }

  /** Resolves once the first connection is open, so no event of a task started after it is missed. */
  ready() {
    return this.opened
  }

  stop() {
    this.controller.abort()
  }

  forTask(root: string) {
    return this.events.filter((event) => event.root_run_id === root)
  }

  private async loop() {
    while (!this.controller.signal.aborted) {
      try {
        const response = await fetch(`${BASE}/events`, {
          headers: this.lastId === null ? {} : { "last-event-id": String(this.lastId) },
          signal: this.controller.signal,
        })
        if (!response.ok || !response.body) throw new Error(`events: ${response.status}`)
        this.markOpen()
        await this.read(response.body)
        this.reconnects.push(`stream ended after id ${this.lastId}`)
      } catch (error) {
        if (this.controller.signal.aborted) return
        this.reconnects.push(message(error))
        await sleep(1000)
      }
    }
  }

  private async read(body: ReadableStream<Uint8Array>) {
    const decoder = new TextDecoder()
    let buffer = ""
    for await (const chunk of body) {
      buffer += decoder.decode(chunk, { stream: true })
      let end: number
      while ((end = buffer.indexOf("\n\n")) !== -1) {
        const frame = buffer.slice(0, end)
        buffer = buffer.slice(end + 2)
        const data = frame
          .split("\n")
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n")
        if (!data) continue
        const event = JSON.parse(data) as EventEnvelope
        if (this.seen.has(event.id)) {
          this.duplicates.push(event.id)
          continue
        }
        this.seen.add(event.id)
        this.events.push(event)
        this.lastId = Math.max(this.lastId ?? 0, event.id)
      }
    }
  }
}
