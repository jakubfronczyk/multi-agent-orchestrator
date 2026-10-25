import { queryOptions } from "@tanstack/react-query"
import type { FollowUpBody } from "@/generated/FollowUpBody"
import type { RunDetail } from "@/generated/RunDetail"
import type { RunId } from "@/generated/RunId"
import type { RunList } from "@/generated/RunList"
import type { StartRunRequest } from "@/generated/StartRunRequest"
import type { StartRunResponse } from "@/generated/StartRunResponse"
import { getJson, postJson, query } from "@/lib/api"

/**
 * The newest tasks, of one team when `teamId` is set. The team joins the key only when set, so
 * a key without it (`["runs", { limit }]`) still matches every team's list for refetching.
 */
export const runsQuery = (limit = 50, teamId?: string) =>
  queryOptions({
    queryKey: ["runs", teamId ? { limit, team_id: teamId } : { limit }],
    queryFn: () => getJson<RunList>(`/runs${query({ limit, team_id: teamId })}`),
  })

export const runQuery = (id: RunId) =>
  queryOptions({
    queryKey: ["run", id],
    queryFn: () => getJson<RunDetail>(`/runs/${id}`),
  })

export const startRun = (request: StartRunRequest) =>
  postJson<StartRunResponse>("/runs", request)

/** 409 while the task runs, or when its root agent is not the Lead. */
export const postFollowUp = (root: RunId, body: FollowUpBody) =>
  postJson<StartRunResponse>(`/runs/${root}/messages`, body)

/** 409 when the task already finished. */
export const cancelRun = (root: RunId) => postJson<void>(`/runs/${root}/cancel`)
