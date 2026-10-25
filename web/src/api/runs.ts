import { queryOptions } from "@tanstack/react-query"
import type { FollowUpBody } from "@/generated/FollowUpBody"
import type { RunDetail } from "@/generated/RunDetail"
import type { RunId } from "@/generated/RunId"
import type { RunList } from "@/generated/RunList"
import type { StartRunRequest } from "@/generated/StartRunRequest"
import type { StartRunResponse } from "@/generated/StartRunResponse"
import { getJson, postJson, query } from "@/lib/api"

export const runsQuery = (limit = 50) =>
  queryOptions({
    queryKey: ["runs", { limit }],
    queryFn: () => getJson<RunList>(`/runs${query({ limit })}`),
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
