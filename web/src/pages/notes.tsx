import { useState } from "react"
import { useQuery } from "@tanstack/react-query"
import { Link, useSearchParams } from "react-router"
import { memoryQuery, memoryVersionsQuery } from "@/api/memory"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { FilterPills } from "@/components/filter-pills"
import { Page } from "@/components/page"
import { TrustPill } from "@/components/trust-pill"
import { useInvalidateOnToolFinished } from "@/events/use-invalidate-on-tool"
import type { MemoryEntryView } from "@/generated/MemoryEntryView"
import type { MemoryScope } from "@/generated/MemoryScope"
import { useAgents } from "@/lib/agents"
import { dayTime } from "@/lib/time"

type Filter = "all" | MemoryScope

const FILTERS: [Filter, string][] = [
  ["all", "All"],
  ["shared", "Shared"],
  ["private", "Private"],
]

export function NotesPage() {
  const [params, setParams] = useSearchParams()
  const raw = params.get("scope")
  const filter: Filter = raw === "shared" || raw === "private" ? raw : "all"
  const notes = useQuery(memoryQuery(filter === "all" ? {} : { scope: filter }))

  useInvalidateOnToolFinished(["memory"], "memory_write")
  useInvalidateOnToolFinished(["memory-versions"], "memory_write")

  const entries = notes.data?.entries ?? []
  return (
    <Page
      title="Team notes"
      lede="Notes your team keeps between tasks. Every teammate can read shared notes; a private note only the teammate it belongs to. Each change is kept as a version with its own trust label. Lead can read only trusted shared notes. Untrusted ones are invisible to it."
    >
      <FilterPills
        label="Show notes"
        options={FILTERS}
        value={filter}
        onChange={(next) => setParams(next === "all" ? {} : { scope: next }, { replace: true })}
      />
      {notes.isPending ? null : notes.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load the notes. {notes.error.message}</p>
      ) : entries.length > 0 ? (
        <div className="flex flex-col gap-3">
          {entries.map((note) => (
            <NoteCard key={note.id} note={note} />
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
          <b className="font-medium">No notes here</b>
          <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
            When a teammate learns something useful, like who owns a system, it saves a short note.
          </p>
        </div>
      )}
    </Page>
  )
}

function NoteCard({ note }: { note: MemoryEntryView }) {
  const look = useAgents()
  const runs = useQuery(runsQuery())
  const [open, setOpen] = useState(false)
  // The history loads only once it is opened.
  const versions = useQuery({ ...memoryVersionsQuery(note.id), enabled: open })
  const author = look.byId(note.author_agent_id)
  const owner = note.owner_agent_id ? look.byId(note.owner_agent_id) : undefined
  const count = note.version

  return (
    <article className="flex flex-col gap-2 rounded-lg bg-card px-5 py-4 ring-1 ring-border">
      <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
        <span className="inline-flex items-center gap-2 font-medium text-sidebar-foreground">
          {author && <AgentShape icon={author.icon} colour={author.colour} size="md" />}
          {author?.name ?? note.author_slug}
        </span>
        <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-sidebar-foreground">
          {note.scope === "shared" ? "Shared with the team" : `Private to ${owner?.name ?? "its teammate"}`}
        </span>
        <TrustPill trust={note.trust} />
        <span className="ml-auto">
          v{note.version}, {dayTime(note.updated_at)}
        </span>
      </div>
      <p className="m-0 text-base break-words">{note.content}</p>
      <small className="font-mono text-xs break-all text-muted-foreground">{note.key}</small>
      {note.scope === "shared" && note.trust === "untrusted" && (
        <span className="text-sm text-amber-800 dark:text-amber-300">
          Lead can't see this note, because it is untrusted.
        </span>
      )}
      <div>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"
        >
          {open ? "Hide history" : `History, ${count} ${count === 1 ? "version" : "versions"}`}
        </button>
      </div>
      {open && (
        <div className="mt-1 flex flex-col border-t border-border">
          {versions.error ? (
            <p className="m-0 py-2.5 text-sm text-destructive">Couldn't load the history. {versions.error.message}</p>
          ) : (
            (versions.data?.versions ?? []).map((version, i) => {
              const task = runs.data?.runs.find((run) => run.id === version.root_run_id)
              return (
              <div
                key={version.version}
                className="grid grid-cols-[32px_minmax(0,1fr)_auto] items-baseline gap-3 border-b border-border py-2.5 last:border-b-0"
              >
                <span className="text-xs text-muted-foreground tabular-nums">v{version.version}</span>
                <div className="min-w-0">
                  <p className={i === 0 ? "m-0 text-sm break-words" : "m-0 text-sm break-words text-sidebar-foreground"}>
                    {version.content}
                  </p>
                  <small className="block text-xs text-muted-foreground">
                    {look.byId(version.author_agent_id)?.name ?? version.author_slug}, {dayTime(version.created_at)}
                    {i === 0 && ", current"}
                    {task && <>, in {task.title ?? task.task}</>}.{" "}
                    <Link
                      to={`/tasks/${version.root_run_id}`}
                      className="text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"
                    >
                      Open the task
                    </Link>
                  </small>
                </div>
                <TrustPill trust={version.trust} />
              </div>
              )
            })
          )}
        </div>
      )}
    </article>
  )
}
