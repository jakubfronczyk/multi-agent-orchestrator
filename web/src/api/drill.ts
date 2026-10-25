import { queryOptions } from "@tanstack/react-query"
import type { DrillReport } from "@/generated/DrillReport"
import type { RunId } from "@/generated/RunId"
import type { StartRunResponse } from "@/generated/StartRunResponse"
import { getJson, postJson } from "@/lib/api"

export const startDrill = () => postJson<StartRunResponse>("/drill")

export const drillReportQuery = (root: RunId) =>
  queryOptions({
    queryKey: ["drill-report", root],
    queryFn: () => getJson<DrillReport>(`/drill/${root}/report`),
  })
