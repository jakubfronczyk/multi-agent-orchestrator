import { type QueryClient, queryOptions } from "@tanstack/react-query"
import type { FileStamp } from "@/generated/FileStamp"
import type { FileWrite } from "@/generated/FileWrite"
import type { FileWritten } from "@/generated/FileWritten"
import type { FileView } from "@/generated/FileView"
import type { TeamId } from "@/generated/TeamId"
import type { UploadResult } from "@/generated/UploadResult"
import { deleteRequest, getJson, postForm, postJson, putJson, query } from "@/lib/api"

/** The workspace as `team`'s agents see it. 422 with `field: "path"` for a bad path or a file too large to show. */
export const fileQuery = (team: TeamId, path: string) =>
  queryOptions({
    queryKey: ["files", team, path],
    queryFn: () => getJson<FileView>(`/files${query({ team_id: team, path })}`),
  })

/**
 * Into the team's `uploads/`: a spreadsheet as one CSV per sheet, a Word document as Markdown, a PDF
 * as text, anything else as sent. 409 every numbered name taken, 413 too big, 422 unreadable; each
 * with `field: "file"`.
 */
export function uploadFile(team: TeamId, file: File) {
  const form = new FormData()
  form.append("file", file)
  return postForm<UploadResult>(`/files${query({ team_id: team })}`, form)
}

/** The person's own save into the team's own folder; without `expected_updated_at` it creates only. */
export const writeFile = (team: TeamId, path: string, input: FileWrite) =>
  putJson<FileWritten>(`/files${query({ team_id: team, path })}`, input)

/** 409 when the file changed since `expectedUpdatedAt`. */
export const deleteFile = (team: TeamId, path: string, expectedUpdatedAt: string) =>
  deleteRequest(`/files${query({ team_id: team, path, expected_updated_at: expectedUpdatedAt })}`)

/** The person vouches for the file, an upload included; tasks it already tainted stay tainted. */
export const markFileTrusted = (team: TeamId, path: string, input: FileStamp) =>
  postJson<void>(`/files/trust${query({ team_id: team, path })}`, input)

/** The person's file changes emit no events, so the lists refetch after each one. */
export const refetchFiles = (client: QueryClient) => client.invalidateQueries({ queryKey: ["files"] })
