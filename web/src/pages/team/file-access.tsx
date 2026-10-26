import { useState } from "react"
import { Folder, Library, Users, type LucideIcon } from "lucide-react"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { fileAccessQuery, setFileAccess } from "@/api/teams"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import type { FileAccess } from "@/generated/FileAccess"
import type { FileAccessView } from "@/generated/FileAccessView"
import type { TeamView } from "@/generated/TeamView"

const CHOICES: [FileAccess, string][] = [
  ["none", "None"],
  ["read", "Read"],
  ["read_write", "Read and write"],
]

/** Which other teams' folders this team's agents may open. Grants emit no events, so a change refetches the list. */
export function FileAccessPanel({ team }: { team: TeamView }) {
  const list = useQuery(fileAccessQuery(team.id))
  const others = list.data?.teams ?? []
  return (
    <section aria-labelledby="file-access" className="flex flex-col gap-1 rounded-lg bg-card p-4 ring-1 ring-border">
      <h3 id="file-access" className="m-0 text-base font-semibold">
        Files {team.name} can open
      </h3>
      <p className="m-0 text-sm text-muted-foreground">
        Its own folder and data/ are always open. Another team's folder opens only if you allow it here.
      </p>
      <ul className="m-0 mt-2 flex list-none flex-col p-0">
        <FixedRow icon={Folder} name="Own folder" path={`${team.name}'s files`} access="Read and write" />
        <FixedRow icon={Library} name="Shared reference" path="data/" access="Read only" />
        {list.error ? (
          <li className="py-3 text-sm text-destructive">Couldn't load file access. {list.error.message}</li>
        ) : list.data && others.length === 0 ? (
          <li className="py-3 text-sm text-muted-foreground">
            No other team yet. Add one with New team in the team menu to share a folder.
          </li>
        ) : (
          others.map((other) => <AccessRow key={other.team_id} team={team} other={other} />)
        )}
      </ul>
      {others.length > 0 && (
        <p className="m-0 mt-1 text-xs text-muted-foreground">
          Read and write saves under the same rules as the team's own folder; a task that read outside content still
          asks first. A change applies to running tasks at their next file step.
        </p>
      )}
    </section>
  )
}

const rowClass = "flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border py-3 last:border-b-0"

/** A folder every agent of the team opens regardless of grants. */
function FixedRow({
  icon: Icon,
  name,
  path,
  access,
}: {
  icon: LucideIcon
  name: string
  path: string
  access: string
}) {
  return (
    <li className={rowClass}>
      <Icon aria-hidden className="size-4 flex-none text-muted-foreground" />
      <span className="min-w-0 flex-1 basis-[160px] text-sm">
        {name}
        <small className="block text-sm break-words text-muted-foreground">{path}</small>
      </span>
      <span className="rounded-md bg-muted px-2.5 py-1 text-sm whitespace-nowrap text-muted-foreground">
        {access}, always
      </span>
    </li>
  )
}

function AccessRow({ team, other }: { team: TeamView; other: FileAccessView }) {
  const client = useQueryClient()
  const [error, setError] = useState<string | null>(null)
  const refetch = () => client.invalidateQueries({ queryKey: fileAccessQuery(team.id).queryKey })
  // Pending until the list has refetched, so the choice never flips back to the old value meanwhile.
  const change = useMutation({
    mutationFn: async (access: FileAccess) => {
      await setFileAccess(team.id, other.team_id, access)
      await refetch()
    },
    onMutate: () => setError(null),
    onError: (failure) => {
      setError(`Couldn't change it. ${failure.message}`)
      void refetch()
    },
  })
  const label = `${other.name}'s files`
  return (
    <li className={rowClass}>
      <Users aria-hidden className="size-4 flex-none text-muted-foreground" />
      <span className="min-w-0 flex-1 basis-[160px] text-sm">
        <span className="break-words">{other.name}</span>
        <small className="block text-sm text-muted-foreground">teams/{other.slug}/</small>
      </span>
      <ToggleGroup
        type="single"
        spacing={0}
        aria-label={label}
        value={change.isPending ? change.variables : other.access}
        // Not disabled while saving: that would drop keyboard focus; a second change waits instead.
        onValueChange={(value) =>
          value && !change.isPending && value !== other.access && change.mutate(value as FileAccess)
        }
        className="grid max-w-full auto-cols-fr grid-flow-col gap-0.5 rounded-md bg-muted p-0.5"
      >
        {CHOICES.map(([value, text]) => (
          <ToggleGroupItem
            key={value}
            value={value}
            className="h-7 rounded-sm px-2.5 text-sm whitespace-nowrap text-sidebar-foreground hover:text-foreground data-[state=on]:bg-card data-[state=on]:font-medium data-[state=on]:text-foreground data-[state=on]:shadow-xs data-[state=on]:ring-1 data-[state=on]:ring-border"
          >
            {text}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {error && (
        <p role="alert" className="m-0 basis-full text-sm text-destructive">
          {error}
        </p>
      )}
    </li>
  )
}
