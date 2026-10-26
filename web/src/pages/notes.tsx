import { useRef, useState } from "react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useSearchParams } from "react-router"
import {
  createNote,
  deleteNote,
  editNote,
  markNoteTrusted,
  memoryQuery,
  memoryVersionsQuery,
  refetchNotes,
} from "@/api/memory"
import { runsQuery } from "@/api/runs"
import { AgentShape } from "@/components/agent-shape"
import { FilterPills } from "@/components/filter-pills"
import { Page } from "@/components/page"
import { TrustPill } from "@/components/trust-pill"
import { Button } from "@/components/ui/button"
import { useInvalidateOnToolFinished } from "@/events/use-invalidate-on-tool"
import type { MemoryEntryView } from "@/generated/MemoryEntryView"
import type { MemoryScope } from "@/generated/MemoryScope"
import { useAgents } from "@/lib/agents"
import { isApiError } from "@/lib/api"
import { useSelectedTeam, useTeamNames } from "@/lib/selected-team"
import { dayTime } from "@/lib/time"

type Filter = "all" | MemoryScope

const action = "text-sm text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"

const FILTERS: [Filter, string][] = [
  ["all", "All"],
  ["shared", "Shared"],
  ["private", "Private"],
]

export function NotesPage() {
  const [params, setParams] = useSearchParams()
  const raw = params.get("scope")
  const filter: Filter = raw === "shared" || raw === "private" ? raw : "all"
  const { team: selected } = useSelectedTeam()
  const names = useTeamNames()
  // `?deleted=` opens a deleted team's notes from the switcher, read-only.
  const deleted = params.get("deleted")
  const teamId = deleted ?? selected?.id
  const teamName = teamId ? names.nameOf(teamId) : undefined
  const unknown = deleted !== null && names.teams.length > 0 && teamName === undefined
  const notes = useQuery({
    ...memoryQuery({ ...(filter === "all" ? {} : { scope: filter }), team_id: teamId }),
    enabled: teamId !== undefined,
  })

  // The list's `last_event_id` also catches writes made while no screen showed it.
  useInvalidateOnToolFinished(["memory"], "memory_write")
  useInvalidateOnToolFinished(["memory-versions"], "memory_write")

  const entries = notes.data?.entries ?? []
  // A deleted card unmounts with the focused button; focus moves to the list instead of the page.
  const list = useRef<HTMLDivElement>(null)
  return (
    <Page
      title="Team notes"
      lede={unknown ? "No team has this id." : "What the team remembers between tasks. The Lead sees only trusted shared notes."}
    >
      {deleted !== null && !unknown && (
        <p className="m-0 text-sm text-muted-foreground">
          Read-only: a deleted team keeps its notes but no longer writes any.{" "}
          <Link
            to={`/tasks?deleted=${deleted}`}
            className="text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"
          >
            Its tasks
          </Link>
        </p>
      )}
      <FilterPills
        label="Show notes"
        options={FILTERS}
        value={filter}
        onChange={(next) =>
          setParams(
            (current) => {
              const updated = new URLSearchParams(current)
              if (next === "all") updated.delete("scope")
              else updated.set("scope", next)
              return updated
            },
            { replace: true },
          )
        }
      />
      {deleted === null && selected && <NewNote key={selected.id} teamId={selected.id} />}
      {notes.isPending ? null : notes.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load the notes. {notes.error.message}</p>
      ) : entries.length > 0 ? (
        <div ref={list} tabIndex={-1} aria-label="Notes" className="flex flex-col gap-3 outline-none">
          {entries.map((note) => (
            <NoteCard
              key={note.id}
              note={note}
              teamName={teamName}
              editable={deleted === null}
              onDeleted={() => list.current?.focus()}
            />
          ))}
        </div>
      ) : (
        <div className="flex flex-col items-center gap-1 rounded-lg px-6 py-10 text-center ring-1 ring-border ring-inset">
          <b className="font-medium">No notes here</b>
          <p className="m-0 max-w-[44ch] text-sm text-muted-foreground">
            When a teammate learns something useful, like who owns a system, it saves a short note. You can add one too.
          </p>
        </div>
      )}
    </Page>
  )
}

/** Why a note change was refused, in the person's words. */
function failure(error: unknown): string {
  if (isApiError(error, 409)) return error.message ? `${error.message[0].toUpperCase()}${error.message.slice(1)}.` : "It changed meanwhile."
  if (isApiError(error, 422) && error.message) return `Check it: ${error.message}.`
  return error instanceof Error ? `Couldn't save it. ${error.message}` : "Couldn't save it."
}

const field =
  "w-full rounded-md bg-card px-3 py-2 text-sm ring-1 ring-border outline-none ring-inset focus-visible:ring-2 focus-visible:ring-ring"

/** A shared note in the person's own words; it is trusted because they wrote it. */
function NewNote({ teamId }: { teamId: string }) {
  const client = useQueryClient()
  const [open, setOpen] = useState(false)
  const [key, setKey] = useState("")
  const [content, setContent] = useState("")
  const [error, setError] = useState<string | null>(null)
  const create = useMutation({
    mutationFn: () => createNote({ team_id: teamId, key: key.trim(), content }),
    onSuccess: async () => {
      await refetchNotes(client)
      setOpen(false)
      setKey("")
      setContent("")
    },
    onError: (failed) => setError(failure(failed)),
  })
  if (!open) {
    return (
      <div>
        <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
          New note
        </Button>
      </div>
    )
  }
  return (
    <form
      className="flex flex-col gap-2 rounded-lg bg-card p-4 ring-1 ring-border"
      onSubmit={(event) => {
        event.preventDefault()
        setError(null)
        if (key.trim() && content.trim() && !create.isPending) create.mutate()
      }}
    >
      <label className="flex flex-col gap-1 text-sm">
        <span className="font-medium">Key</span>
        <input
          autoFocus
          value={key}
          onChange={(event) => setKey(event.target.value)}
          placeholder="finance/cfo"
          maxLength={128}
          className={`${field} font-mono`}
        />
      </label>
      <label className="flex flex-col gap-1 text-sm">
        <span className="font-medium">Note</span>
        <textarea
          value={content}
          onChange={(event) => setContent(event.target.value)}
          rows={3}
          placeholder="Invoices over 5,000 EUR go to Marta Lis (marta.lis@example.com)."
          className={field}
        />
      </label>
      <p className="m-0 text-sm text-muted-foreground">
        Shared with the team and trusted, because you wrote it. The Lead can read it.
      </p>
      {error && (
        <p role="alert" className="m-0 text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={!key.trim() || !content.trim() || create.isPending}>
          Save note
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setOpen(false)}>
          Cancel
        </Button>
      </div>
    </form>
  )
}

function NoteCard({
  note,
  teamName,
  editable,
  onDeleted,
}: {
  note: MemoryEntryView
  teamName: string | undefined
  editable: boolean
  onDeleted: () => void
}) {
  const look = useAgents()
  const client = useQueryClient()
  const [mode, setMode] = useState<"view" | "edit" | "delete" | "trust">("view")
  const [draft, setDraft] = useState(note.content)
  // The version the person saw when they opened an action. The list refetches while an action is
  // open, so `note.version` may already be newer; sending it would accept text they never read.
  const [seen, setSeen] = useState(note.version)
  const [error, setError] = useState<string | null>(null)
  const card = useRef<HTMLElement>(null)
  const done = async () => {
    await refetchNotes(client)
    setMode("view")
    card.current?.focus()
  }
  const onError = async (failed: unknown) => {
    setError(failure(failed))
    // A 409 means someone wrote a newer version: close the action and show that version.
    if (isApiError(failed, 409)) {
      setMode("view")
      await refetchNotes(client)
      card.current?.focus()
    }
  }
  const save = useMutation({
    mutationFn: () => editNote(note.id, { content: draft, expected_version: seen }),
    onSuccess: done,
    onError,
  })
  const remove = useMutation({
    mutationFn: () => deleteNote(note.id, seen),
    onSuccess: async () => {
      await refetchNotes(client)
      onDeleted()
    },
    onError,
  })
  const vouch = useMutation({
    mutationFn: () => markNoteTrusted(note.id, { expected_version: seen }),
    onSuccess: done,
    onError,
  })
  const shared = note.scope === "shared"
  const start = (next: "edit" | "delete" | "trust") => {
    setError(null)
    setDraft(note.content)
    setSeen(note.version)
    setMode(next)
  }
  const runs = useQuery(runsQuery())
  const [open, setOpen] = useState(false)
  // The history loads only once it is opened.
  const versions = useQuery({ ...memoryVersionsQuery(note.id), enabled: open })
  const author = note.author_agent_id ? look.byId(note.author_agent_id) : undefined
  const owner = note.owner_agent_id ? look.byId(note.owner_agent_id) : undefined
  const count = note.version

  return (
    <article
      ref={card}
      tabIndex={-1}
      aria-label={note.key}
      className="flex flex-col gap-2 rounded-lg bg-card px-5 py-4 ring-1 ring-border outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
        <span className="inline-flex items-center gap-2 font-medium text-sidebar-foreground">
          {author && <AgentShape icon={author.icon} colour={author.colour} size="md" />}
          {note.author_agent_id === null ? "You" : (author?.name ?? note.author_slug)}
        </span>
        <span className="rounded-full bg-muted px-2 py-0.5 text-xs text-sidebar-foreground">
          {note.scope === "shared" ? `Shared with ${teamName ?? "the team"}` : `Private to ${owner?.name ?? "its teammate"}`}
        </span>
        <TrustPill trust={note.trust} />
        <span className="ml-auto">
          v{note.version}, {dayTime(note.updated_at)}
        </span>
      </div>
      {mode === "edit" ? (
        <form
          className="flex flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            setError(null)
            if (draft.trim() && !save.isPending) save.mutate()
          }}
        >
          <textarea
            autoFocus
            aria-label="Note"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            rows={3}
            className={field}
          />
          {note.trust === "untrusted" && (
            <p className="m-0 text-sm text-muted-foreground">
              Saving keeps it untrusted: it may still hold outside text. Mark it as trusted separately once you've checked it.
            </p>
          )}
          <div className="flex gap-2">
            <Button type="submit" size="sm" disabled={!draft.trim() || save.isPending}>
              Save
            </Button>
            <Button type="button" size="sm" variant="ghost" onClick={() => setMode("view")}>
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <p className="m-0 text-base break-words whitespace-pre-wrap">{note.content}</p>
      )}
      <small className="font-mono text-xs break-all text-muted-foreground">{note.key}</small>
      {note.scope === "shared" && note.trust === "untrusted" && (
        <span className="text-sm text-amber-800 dark:text-amber-300">
          Lead can't see this note, because it is untrusted.
        </span>
      )}
      {mode === "trust" && (
        <div role="alertdialog" aria-label="Mark as trusted" className="flex flex-col gap-2 rounded-md bg-muted px-4 py-3 text-sm">
          <span>
            Agents, the Lead included, will treat this note as your own words. Only do this if you've read it and nothing in
            it gives instructions. Tasks that already read it stay as they are.
          </span>
          <div className="flex gap-2">
            <Button size="sm" disabled={vouch.isPending} onClick={() => vouch.mutate()}>
              Mark as trusted
            </Button>
            <Button size="sm" variant="outline" autoFocus onClick={() => setMode("view")}>
              Keep untrusted
            </Button>
          </div>
        </div>
      )}
      {mode === "delete" && (
        <div
          role="alertdialog"
          aria-label="Delete note"
          className="flex flex-wrap items-center gap-2 rounded-md bg-muted px-4 py-3 text-sm"
        >
          <span className="min-w-[200px] flex-1">Delete this note? Agents won't see it unless one writes this key again; its history stays.</span>
          <Button size="sm" disabled={remove.isPending} onClick={() => remove.mutate()}>
            Delete
          </Button>
          <Button size="sm" variant="outline" autoFocus onClick={() => setMode("view")}>
            Keep
          </Button>
        </div>
      )}
      {error && (
        <p role="alert" className="m-0 text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
        {note.root_run_id && (
          <Link to={`/tasks/${note.root_run_id}`} className={action}>
            Open the task that wrote it
          </Link>
        )}
        {editable && mode === "view" && (
          <>
            {shared && (
              <button type="button" onClick={() => start("edit")} className={action}>
                Edit
              </button>
            )}
            {shared && note.trust === "untrusted" && (
              <button type="button" onClick={() => start("trust")} className={action}>
                Mark as trusted
              </button>
            )}
            <button type="button" onClick={() => start("delete")} className={`${action} text-destructive`}>
              Delete
            </button>
          </>
        )}
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
                    {version.author_agent_id === null
                      ? "You"
                      : (look.byId(version.author_agent_id)?.name ?? version.author_slug)}
                    , {dayTime(version.created_at)}
                    {i === 0 && ", current"}
                    {task && <>, in {task.title ?? task.task}</>}.{" "}
                    {version.root_run_id && (
                      <Link
                        to={`/tasks/${version.root_run_id}`}
                        className="text-sidebar-foreground underline decoration-input underline-offset-[3px] hover:text-foreground"
                      >
                        Open the task
                      </Link>
                    )}
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
