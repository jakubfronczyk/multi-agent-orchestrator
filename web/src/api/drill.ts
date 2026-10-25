import { queryOptions } from "@tanstack/react-query"
import type { DrillReport } from "@/generated/DrillReport"
import type { RunId } from "@/generated/RunId"
import type { StartRunResponse } from "@/generated/StartRunResponse"
import { getJson, postJson, query } from "@/lib/api"

/** 409 when the team's Lead cannot hand work to the agents the drill needs. */
export const startDrill = (teamId?: string) => postJson<StartRunResponse>(`/drill${query({ team_id: teamId })}`)

export const drillReportQuery = (root: RunId) =>
  queryOptions({
    queryKey: ["drill-report", root],
    queryFn: () => getJson<DrillReport>(`/drill/${root}/report`),
  })
