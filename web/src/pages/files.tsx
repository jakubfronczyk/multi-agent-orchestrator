import { useEffect, useRef, useState } from "react"
import { useQueries, useQuery } from "@tanstack/react-query"
import { Link, useSearchParams } from "react-router"
import { ChevronRight, CircleCheck, FileText, Folder, TriangleAlert } from "lucide-react"
import { cn } from "cn"
import { fileQuery } from "@/api/files"
import { runsQuery } from "@/api/runs"
import { Page } from "@/components/page"
import { TrustPill } from "@/components/trust-pill"
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

const parentOf = (path: string) => (path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : ".")

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
}

export function FilesPage() {
  const { team } = useSelectedTeam()
  const [, setParams] = useSearchParams()
  // A path names a file in one team's view, so another team starts from its own first file.
  const shown = useRef(team?.id)
  useEffect(() => {
    if (shown.current !== undefined && shown.current !== team?.id) setParams({}, { replace: true })
    shown.current = team?.id
  }, [team?.id, setParams])
  return (
    <Page
      wide
      title="Files"
      lede={`What ${team?.name ?? "this team"}'s agents can open: their own folder, data/ (shared reference every team reads and no agent can change), and teams/ for any folder another team lets them open. Every file carries a trust label: a file saved after reading outside content stays untrusted, and the label follows it into any task that reads it.`}
    >
      {team && <Workspace key={team.id} team={team.id} />}
    </Page>
  )
}

function Workspace({ team }: { team: TeamId }) {
  const [params, setParams] = useSearchParams()
  const root = useQuery(folderQuery(team, "."))
  const topDirs = sorted(entriesOf(root.data)).filter((entry) => entry.kind === "dir")
  const topLists = useQueries({ queries: topDirs.map((dir) => folderQuery(team, dir.path)) })

  useInvalidateOnToolFinished(["files"], "file_write")

  // The first file in tree order, once the top of the tree has loaded.
  const firstFile =
    sorted(entriesOf(root.data)).find((entry) => entry.kind !== "dir")?.path ??
    topLists.map((list) => sorted(entriesOf(list.data)).find((entry) => entry.kind !== "dir")?.path).find(Boolean)
  const selected = params.get("path") ?? firstFile ?? null
  const select = (path: string) => setParams({ path }, { replace: true })

  return (
    <>
      <div className="grid grid-cols-1 items-start gap-4 md:grid-cols-[minmax(0,300px)_minmax(0,1fr)]">
        <div className="rounded-lg bg-card px-1 pt-1 pb-2 ring-1 ring-border">
          {root.error ? (
            <p className="m-0 p-3 text-sm text-destructive">Couldn't load the files. {root.error.message}</p>
          ) : root.data && entriesOf(root.data).length === 0 ? (
            <p className="m-0 p-3 text-sm text-muted-foreground">This team's folder is empty.</p>
          ) : (
            sorted(entriesOf(root.data)).map((entry) => (
              <TreeEntry key={entry.path} team={team} entry={entry} depth={0} selected={selected} onSelect={select} startOpen />
            ))
          )}
        </div>
        <div className="rounded-lg bg-card ring-1 ring-border">
          {selected ? (
            <Preview team={team} path={selected} />
          ) : (
            <p className="m-0 p-5 text-sm text-muted-foreground">Pick a file to see it here.</p>
          )}
        </div>
      </div>
    </>
  )
}

function TreeEntry({
  team,
  entry,
  depth,
  selected,
  onSelect,
  startOpen = false,
}: {
  team: TeamId
  entry: FileEntryView
  depth: number
  selected: string | null
  onSelect: (path: string) => void
  startOpen?: boolean
}) {
  const [open, setOpen] = useState(startOpen)
  const isDir = entry.kind === "dir"
  // Folders load when opened; a deep link keeps every folder above the file open.
  const expanded = isDir && (open || (selected?.startsWith(`${entry.path}/`) ?? false))
  const list = useQuery({ ...folderQuery(team, entry.path), enabled: expanded })
  const names = useTeamNames()
  // A granted folder is named by its team's slug; the team's name reads better.
  const owner = /^teams\/[^/]+$/.test(entry.path) ? names.teams.find((t) => t.deleted_at === null && `teams/${t.slug}` === entry.path) : undefined
  const label = owner ? `${owner.name} (${entry.name}/)` : entry.path === "data" ? "data/ (shared, read only)" : `${entry.name}/`
  const pad = { paddingLeft: `${12 + depth * 16}px` }

  if (!isDir) {
    return (
      <button
        type="button"
        aria-current={selected === entry.path}
        onClick={() => onSelect(entry.path)}
        style={pad}
        className={cn(
          "flex h-9 w-full items-center gap-2 rounded-md pr-3 text-left text-sm text-sidebar-foreground hover:bg-accent hover:text-foreground",
          selected === entry.path && "bg-accent text-foreground",
        )}
      >
        <FileText className="size-4 flex-none" />
        <span className="min-w-0 flex-1 truncate">{entry.name}</span>
        <TrustPill trust={entry.trust} />
      </button>
    )
  }
  return (
    <div>
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setOpen(!expanded)}
        style={pad}
        className="flex h-9 w-full items-center gap-2 rounded-md pr-3 text-left text-sm font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
      >
        <ChevronRight className={cn("size-4 flex-none transition-transform", expanded && "rotate-90")} />
        <Folder className="size-4 flex-none" />
        <span className="min-w-0 flex-1 truncate">{label}</span>
        <TrustPill trust={entry.trust} />
      </button>
      {expanded &&
        (list.error ? (
          <p className="m-0 py-1.5 text-sm text-destructive" style={{ paddingLeft: `${28 + depth * 16}px` }}>
            {list.error.message}
          </p>
        ) : (
          sorted(entriesOf(list.data)).map((child) => (
            <TreeEntry key={child.path} team={team} entry={child} depth={depth + 1} selected={selected} onSelect={onSelect} />
          ))
        ))}
    </div>
  )
}

function Preview({ team, path }: { team: TeamId; path: string }) {
  const file = useQuery({ ...fileQuery(team, path), refetchOnMount: "always" })
  const parent = useQuery(folderQuery(team, parentOf(path)))
  const runs = useQuery(runsQuery())
  const names = useTeamNames()
  const entry = entriesOf(parent.data).find((e) => e.path === path)

  if (file.error) {
    return (
      <div className="flex flex-col gap-2 p-5">
        <b className="font-medium break-all">{path}</b>
        <p className="m-0 text-sm text-destructive">
          {isApiError(file.error, 404) ? "This file doesn't exist." : file.error.message}
        </p>
      </div>
    )
  }
  if (!file.data) return null
  if (file.data.kind === "dir") {
    return <p className="m-0 p-5 text-sm text-muted-foreground">{path}/ is a folder. Pick a file inside it.</p>
  }
  const view = file.data
  const upload = isUpload(view.path)
  const writer = view.written_by_root_run_id
  const task = writer ? runs.data?.runs.find((run) => run.id === writer) : undefined
  const why = upload
    ? "You uploaded it, so it counts as outside content. Any task that reads it is treated as having read outside content too."
    : view.trust === "trusted"
      ? "Part of your workspace. No task that read outside content has written it."
      : "Written by a task that had read outside content. Any task that reads it is treated as having read outside content too."

  return (
    <div className="flex min-w-0 flex-col gap-3 p-5">
      <div className="flex flex-wrap items-center gap-3">
        <b className="font-medium break-all">{view.path}</b>
        {entry && <small className="text-sm text-muted-foreground">{size(entry.bytes)}</small>}
        <TrustPill trust={view.trust} className="ml-auto" />
      </div>
      <p className="m-0 flex gap-2 text-sm text-sidebar-foreground">
        {view.trust === "trusted" ? (
          <CircleCheck className="mt-0.5 size-4 flex-none text-emerald-700 dark:text-emerald-400" />
        ) : (
          <TriangleAlert className="mt-0.5 size-4 flex-none text-amber-800 dark:text-amber-300" />
        )}
        <span>{why}</span>
      </p>
      {writer && (
        <p className="m-0 text-sm text-sidebar-foreground">
          Saved by {task ? (task.title ?? task.task) : "a task"}
          {view.written_by_team_id && names.nameOf(view.written_by_team_id) && <>, a task of {names.nameOf(view.written_by_team_id)}</>}.{" "}
          <Link to={`/tasks/${writer}`} className="underline decoration-input underline-offset-[3px] hover:text-foreground">
            Open the task that saved it
          </Link>
        </p>
      )}
      <pre className="m-0 max-h-[420px] overflow-auto rounded-md bg-muted p-4 font-mono text-sm/relaxed break-words whitespace-pre-wrap text-sidebar-foreground">
        {view.content}
      </pre>
    </div>
  )
}
