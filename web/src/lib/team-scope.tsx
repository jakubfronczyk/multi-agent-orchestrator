import { useSearchParams } from "react-router"
import { FilterPills } from "@/components/filter-pills"
import { useSelectedTeam, useTeamNames } from "@/lib/selected-team"

type Scope = "team" | "all"

/**
 * History pages show the selected team by default, every team with `?team=all`, or one deleted
 * team with `?deleted=`. The team goes into the list query, so the page limit applies within it.
 */
export function useTeamScope() {
  const { team } = useSelectedTeam()
  const names = useTeamNames()
  const [params, setParams] = useSearchParams()
  const deleted = params.get("deleted")
  const scope: Scope = params.get("team") === "all" ? "all" : "team"
  const setScope = (next: Scope) =>
    setParams(
      (current) => {
        const updated = new URLSearchParams(current)
        if (next === "all") updated.set("team", "all")
        else updated.delete("team")
        return updated
      },
      { replace: true },
    )
  return {
    scope,
    team,
    /** The deleted team shown read-only, by id; null for live history. */
    deleted,
    deletedName: deleted === null ? undefined : names.nameOf(deleted),
    /** The `team_id` to query with; undefined under "All teams". */
    teamId: deleted ?? (scope === "all" ? undefined : team?.id),
    /** False until the selected team is known, so no list is fetched for the wrong team. */
    ready: deleted !== null || scope === "all" || team !== undefined,
    /** A record's team name, shown under "All teams" when there is more than one team. */
    label: (teamId: string) => (deleted === null && scope === "all" && names.many ? names.nameOf(teamId) : undefined),
    pills: team && names.many && deleted === null ? (
      <FilterPills
        label="Team"
        showLabel
        options={[
          ["team", team.name],
          ["all", "All teams"],
        ]}
        value={scope}
        onChange={setScope}
      />
    ) : null,
  }
}
