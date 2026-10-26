import { queryOptions } from "@tanstack/react-query"
import type { FileView } from "@/generated/FileView"
import type { TeamId } from "@/generated/TeamId"
import type { UploadResult } from "@/generated/UploadResult"
import { getJson, postForm, query } from "@/lib/api"

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
