import { type QueryClient, queryOptions } from "@tanstack/react-query"
import type { FileAccess } from "@/generated/FileAccess"
import type { FileAccessList } from "@/generated/FileAccessList"
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

/** Refetches teams and agents after a team or agent write, since neither emits events. */
export const refetchTeams = (client: QueryClient) =>
  Promise.all([client.invalidateQueries({ queryKey: ["teams"] }), client.invalidateQueries({ queryKey: ["agents"] })])

export const createTeam = (input: TeamInput) => postJson<TeamView>("/teams", input)

export const renameTeam = (id: TeamId, input: TeamInput) => putJson<TeamView>(`/teams/${id}`, input)

/** Every other live team and how much of its folder `team`'s agents may open. */
export const fileAccessQuery = (team: TeamId) =>
  queryOptions({
    queryKey: ["teams", "file-access", team],
    queryFn: () => getJson<FileAccessList>(`/teams/${team}/file-access`),
  })

/** Applies at a running task's next file call; `none` removes the grant. */
export const setFileAccess = (team: TeamId, of: TeamId, access: FileAccess) =>
  putJson<void>(`/teams/${team}/file-access/${of}`, { access })

/** 409 while a task of the team is live, or when it is the last team. */
export const deleteTeam = (id: TeamId) => deleteRequest(`/teams/${id}`)
