import { queryOptions } from "@tanstack/react-query"
import type { FileView } from "@/generated/FileView"
import type { UploadResponse } from "@/generated/UploadResponse"
import { getJson, postForm, query } from "@/lib/api"

/** 422 with `field: "path"` for a bad path or a file too large to show. */
export const fileQuery = (path: string) =>
  queryOptions({
    queryKey: ["files", path],
    queryFn: () => getJson<FileView>(`/files${query({ path })}`),
  })

/** 409 duplicate, 413 too big, 415 not UTF-8, 422 bad name; each with `field: "file"`. */
export function uploadFile(file: File) {
  const form = new FormData()
  form.append("file", file)
  return postForm<UploadResponse>("/files", form)
}
