import { useEffect, useRef, useState } from "react"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { Link, useSearchParams } from "react-router"
import { ChevronRight, Download, FileText, Folder } from "lucide-react"
import { cn } from "cn"
import { fileQuery } from "@/api/files"
import { runsQuery } from "@/api/runs"
import { Page } from "@/components/page"
import { useInvalidateOnToolFinished } from "@/events/use-invalidate-on-tool"
import type { FileEntryView } from "@/generated/FileEntryView"
import type { FileView } from "@/generated/FileView"
import type { TeamId } from "@/generated/TeamId"
import { isApiError } from "@/lib/api"
import { useSelectedTeam, useTeamNames } from "@/lib/selected-team"

// A file list has no `last_event_id`, so a folder refetches whenever it is shown again.
const folderQuery = (team: TeamId, path: string) => ({ ...fileQuery(team, path), refetchOnMount: "always" as const })

/** An upload sits in a team's `uploads/`: this team's own, or one under `teams/<slug>/`. */
const isUpload = (path: string) => /^(teams\/[^/]+\/)?uploads\//.test(path)

const entriesOf = (view: FileView | undefined): FileEntryView[] => (view?.kind === "dir" ? view.entries : [])

/** Folders first, then files, each by name; symlinks count as files. */
const sorted = (entries: FileEntryView[]) =>
  [...entries].sort((a, b) => Number(b.kind === "dir") - Number(a.kind === "dir") || a.name.localeCompare(b.name))

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
}

/** Hands `content` to the browser as a file named `name`. */
function saveAs(name: string, content: string) {
  const url = URL.createObjectURL(new Blob([content], { type: "text/plain;charset=utf-8" }))
  const link = document.createElement("a")
  link.href = url
  link.download = name
  link.click()
  setTimeout(() => URL.revokeObjectURL(url))
}

export function FilesPage() {
  const { team } = useSelectedTeam()
  const [, setParams] = useSearchParams()
  // A path names a file in one team's view, so another team drops it.
  const shown = useRef(team?.id)
  useEffect(() => {
    if (shown.current !== undefined && shown.current !== team?.id) setParams({}, { replace: true })
    shown.current = team?.id
  }, [team?.id, setParams])
  return (
    <Page
      wide
      title="Files"
      lede={`What ${team?.name ?? "this team"}'s agents can open. Untrusted marks a file saved after reading outside content.`}
    >
      {team && <Workspace key={team.id} team={team.id} />}
    </Page>
  )
}

function Workspace({ team }: { team: TeamId }) {
  const [params] = useSearchParams()
  const root = useQuery(folderQuery(team, "."))
  useInvalidateOnToolFinished(["files"], "file_write")
  // A task links here with `?path=`; that row is highlighted, not opened.
  const highlighted = params.get("path")

  return (
    <div className="flex flex-col gap-0.5 rounded-lg bg-card p-2 ring-1 ring-border">
      {root.error ? (
        <p className="m-0 p-3 text-sm text-destructive">Couldn't load the files. {root.error.message}</p>
      ) : root.data && entriesOf(root.data).length === 0 ? (
        <p className="m-0 p-3 text-sm text-muted-foreground">This team's folder is empty.</p>
      ) : (
        sorted(entriesOf(root.data)).map((entry) => (
          <TreeEntry key={entry.path} team={team} entry={entry} depth={0} highlighted={highlighted} />
        ))
      )}
    </div>
  )
}

function TreeEntry({
  team,
  entry,
  depth,
  highlighted,
}: {
  team: TeamId
  entry: FileEntryView
  depth: number
  highlighted: string | null
}) {
  const [open, setOpen] = useState(false)
  // A top-level folder is a fixed section heading; deeper folders toggle.
  const section = entry.kind === "dir" && depth === 0
  // Folders load when opened; a linked file keeps every folder above it open.
  const expanded =
    entry.kind === "dir" && (section || open || (highlighted?.startsWith(`${entry.path}/`) ?? false))
  const list = useQuery({ ...folderQuery(team, entry.path), enabled: expanded })
  const names = useTeamNames()
  // A granted folder is named by its team's slug; the team's name reads better.
  const owner = /^teams\/[^/]+$/.test(entry.path)
    ? names.teams.find((t) => t.deleted_at === null && `teams/${t.slug}` === entry.path)
    : undefined
  const label = owner ? owner.name : `${entry.name}/`
  const note = owner ? `${entry.name}/` : entry.path === "data" ? "read only" : null
  const pad = { paddingLeft: `${8 + Math.max(0, depth - 1) * 16}px` }

  if (entry.kind !== "dir") return <FileRow team={team} entry={entry} pad={pad} highlighted={highlighted === entry.path} />

  const children =
    expanded &&
    (list.error ? (
      <p className="m-0 py-1.5 text-sm text-destructive" style={{ paddingLeft: `${8 + depth * 16}px` }}>
        {list.error.message}
      </p>
    ) : list.data && entriesOf(list.data).length === 0 ? (
      <p className="m-0 py-1.5 text-sm text-muted-foreground" style={{ paddingLeft: `${8 + depth * 16}px` }}>
        Empty
      </p>
    ) : (
      sorted(entriesOf(list.data)).map((child) => (
        <TreeEntry key={child.path} team={team} entry={child} depth={depth + 1} highlighted={highlighted} />
      ))
    ))

  if (section) {
    return (
      <div role="group" aria-label={label} className="flex flex-col gap-0.5 not-first:mt-4">
        <div className="flex items-center gap-1.5 px-2 pt-1 pb-1.5 text-xs font-medium text-muted-foreground">
          <Folder className="size-3.5 flex-none" />
          <span className="min-w-0 truncate">{label}</span>
          <span className="min-w-0 flex-1 truncate font-normal text-muted-foreground/70">{note}</span>
        </div>
        {children}
      </div>
    )
  }
  return (
    <div className="flex flex-col gap-0.5">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setOpen(!expanded)}
        style={pad}
        className="flex h-9 w-full items-center gap-2 rounded-md pr-2 text-left text-sm text-muted-foreground hover:text-foreground"
      >
        <ChevronRight className={cn("size-4 flex-none transition-transform", expanded && "rotate-90")} />
        <span className="min-w-0 truncate">{label}</span>
      </button>
      {children}
    </div>
  )
}

function FileRow({
  team,
  entry,
  pad,
  highlighted,
}: {
  team: TeamId
  entry: FileEntryView
  pad: { paddingLeft: string }
  highlighted: boolean
}) {
  const client = useQueryClient()
  const runs = useQuery(runsQuery())
  const names = useTeamNames()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const row = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (highlighted) row.current?.scrollIntoView({ block: "center" })
  }, [highlighted])

  const writer = entry.written_by_root_run_id
  const task = writer ? runs.data?.runs.find((run) => run.id === writer) : undefined
  const writerTeam = entry.written_by_team_id ? names.nameOf(entry.written_by_team_id) : null
  const why = isUpload(entry.path)
    ? "You uploaded it, so it counts as outside content."
    : "Saved by a task that had read outside content."

  const download = async () => {
    setBusy(true)
    setError(null)
    try {
      const view = await client.fetchQuery({ ...fileQuery(team, entry.path), staleTime: 0 })
      if (view.kind === "file") saveAs(entry.name, view.content)
    } catch (failure) {
      setError(isApiError(failure, 422) ? "Too large to download here." : `Couldn't download it. ${(failure as Error).message}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      ref={row}
      style={pad}
      className={cn(
        "flex min-h-10 flex-wrap items-center gap-x-3 gap-y-0.5 rounded-md py-1 pr-1 text-sm",
        highlighted && "bg-accent",
      )}
    >
      <FileText className="size-4 flex-none text-muted-foreground" />
      <span className="min-w-0 flex-1 basis-[140px] truncate text-foreground">{entry.name}</span>
      {entry.trust === "untrusted" && (
        <span title={why} className="flex-none text-xs font-medium text-amber-800 dark:text-amber-300">
          Untrusted
        </span>
      )}
      {writer && (
        <Link
          to={`/tasks/${writer}`}
          className="hidden max-w-[220px] flex-none truncate text-xs text-muted-foreground hover:text-foreground sm:block"
        >
          {task ? (task.title ?? task.task) : "Saved by a task"}
          {writerTeam && <>, {writerTeam}</>}
        </Link>
      )}
      <span className="w-14 flex-none text-right text-xs text-muted-foreground tabular-nums">{size(entry.bytes)}</span>
      <button
        type="button"
        aria-label={`Download ${entry.name}`}
        title="Download"
        disabled={busy}
        onClick={download}
        className="grid size-8 flex-none place-items-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-50"
      >
        <Download className="size-4" />
      </button>
      {error && (
        <p role="alert" className="m-0 basis-full pl-7 text-xs text-destructive">
          {error}
        </p>
      )}
    </div>
  )
}
