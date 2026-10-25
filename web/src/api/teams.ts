import { queryOptions } from "@tanstack/react-query"
import type { TeamId } from "@/generated/TeamId"
import type { TeamInput } from "@/generated/TeamInput"
import type { TeamList } from "@/generated/TeamList"
import type { TeamView } from "@/generated/TeamView"
import { deleteRequest, getJson, postJson, putJson, query } from "@/lib/api"

/** Team CRUD emits no events; every write refetches this list itself. */
export const teamsQuery = (includeDeleted = false) =>
  queryOptions({
    queryKey: ["teams", { includeDeleted }],
    queryFn: () => getJson<TeamList>(`/teams${query({ include_deleted: includeDeleted ? "true" : undefined })}`),
  })

export const createTeam = (input: TeamInput) => postJson<TeamView>("/teams", input)

export const renameTeam = (id: TeamId, input: TeamInput) => putJson<TeamView>(`/teams/${id}`, input)

/** 409 while a task of the team is live, or when it is the last team. */
export const deleteTeam = (id: TeamId) => deleteRequest(`/teams/${id}`)
