import { useState } from "react"
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
    <section aria-labelledby="file-access" className="flex flex-col gap-3 rounded-lg bg-card p-4 ring-1 ring-border">
      <div className="flex flex-col gap-1">
        <h3 id="file-access" className="m-0 text-base font-semibold">
          Files {team.name} can open
        </h3>
        <p className="m-0 text-sm text-muted-foreground">
          {team.name}'s agents always open their own folder and data/. They open another team's folder only if you allow
          it here, as teams/&lt;its short name&gt;/. Read and write lets them replace that team's files without asking;
          a task that read outside content still asks first. A change applies to running tasks at their next file step.
        </p>
      </div>
      {list.error ? (
        <p className="m-0 text-sm text-destructive">Couldn't load file access. {list.error.message}</p>
      ) : list.data && others.length === 0 ? (
        <p className="m-0 text-sm text-muted-foreground">There is no other team yet.</p>
      ) : (
        <ul className="m-0 flex list-none flex-col gap-2 p-0">
          {others.map((other) => (
            <AccessRow key={other.team_id} team={team} other={other} />
          ))}
        </ul>
      )}
    </section>
  )
}

function AccessRow({ team, other }: { team: TeamView; other: FileAccessView }) {
  const client = useQueryClient()
  const [error, setError] = useState<string | null>(null)
  const change = useMutation({
    mutationFn: (access: FileAccess) => setFileAccess(team.id, other.team_id, access),
    onMutate: () => setError(null),
    onSettled: () => client.invalidateQueries({ queryKey: fileAccessQuery(team.id).queryKey }),
    onError: (failure) => setError(`Couldn't change it. ${failure.message}`),
  })
  const label = `${other.name}'s files`
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <span className="min-w-0 flex-1 basis-[180px] text-sm">
        <span className="font-medium break-words">{other.name}</span>{" "}
        <span className="text-muted-foreground">teams/{other.slug}/</span>
      </span>
      <ToggleGroup
        type="single"
        spacing={0}
        aria-label={label}
        value={change.isPending ? change.variables : other.access}
        disabled={change.isPending}
        onValueChange={(value) => value && value !== other.access && change.mutate(value as FileAccess)}
        className="grid grid-cols-3 gap-0.5 rounded-md bg-secondary p-0.5"
      >
        {CHOICES.map(([value, text]) => (
          <ToggleGroupItem
            key={value}
            value={value}
            className="h-7 rounded-sm px-2.5 text-sm whitespace-nowrap text-sidebar-foreground data-[state=on]:bg-card data-[state=on]:font-medium data-[state=on]:text-foreground data-[state=on]:shadow-xs data-[state=on]:ring-1 data-[state=on]:ring-border"
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
