import { useSearchParams } from "react-router"
import { FilterPills } from "@/components/filter-pills"
import { useSelectedTeam, useTeamNames } from "@/lib/selected-team"

type Scope = "team" | "all"

/**
 * History pages show the selected team by default, or every team with `?team=all`. Records are
 * filtered by their own `team_id`, so the lists and their event refetches stay global.
 */
export function useTeamScope() {
  const { team } = useSelectedTeam()
  const names = useTeamNames()
  const [params, setParams] = useSearchParams()
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
    /** Whether a record of this team is shown. */
    shows: (teamId: string) => scope === "all" || teamId === team?.id,
    /** A record's team name, shown under "All teams" when there is more than one team. */
    label: (teamId: string) => (scope === "all" && names.many ? names.nameOf(teamId) : undefined),
    pills: team ? (
      <FilterPills
        label="Team"
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
