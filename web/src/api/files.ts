import { queryOptions } from "@tanstack/react-query"
import type { FileView } from "@/generated/FileView"
import type { TeamId } from "@/generated/TeamId"
import type { UploadResponse } from "@/generated/UploadResponse"
import { getJson, postForm, query } from "@/lib/api"

/** The workspace as `team`'s agents see it. 422 with `field: "path"` for a bad path or a file too large to show. */
export const fileQuery = (team: TeamId, path: string) =>
  queryOptions({
    queryKey: ["files", team, path],
    queryFn: () => getJson<FileView>(`/files${query({ team_id: team, path })}`),
  })

/** Into the team's `uploads/`. 409 duplicate, 413 too big, 415 not UTF-8, 422 bad name; each with `field: "file"`. */
export function uploadFile(team: TeamId, file: File) {
  const form = new FormData()
  form.append("file", file)
  return postForm<UploadResponse>(`/files${query({ team_id: team })}`, form)
}
