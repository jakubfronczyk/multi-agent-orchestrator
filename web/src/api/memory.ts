import { type QueryClient, queryOptions } from "@tanstack/react-query"
import type { MemoryEntryView } from "@/generated/MemoryEntryView"
import type { MemoryList } from "@/generated/MemoryList"
import type { MemoryScope } from "@/generated/MemoryScope"
import type { MemoryVersionList } from "@/generated/MemoryVersionList"
import type { NoteCreate } from "@/generated/NoteCreate"
import type { NoteEdit } from "@/generated/NoteEdit"
import type { NoteVersion } from "@/generated/NoteVersion"
import { deleteRequest, getJson, postJson, putJson, query } from "@/lib/api"

/** Filters match the server's `GET /memory` query parameters. */
export const memoryQuery = (
  filter: { scope?: MemoryScope } & Partial<Record<"agent_id" | "q" | "team_id", string>> = {},
) =>
  queryOptions({
    queryKey: ["memory", filter],
    queryFn: () => getJson<MemoryList>(`/memory${query(filter)}`),
  })

export const memoryVersionsQuery = (entryId: string) =>
  queryOptions({
    queryKey: ["memory-versions", entryId],
    queryFn: () => getJson<MemoryVersionList>(`/memory/${entryId}/versions`),
    refetchOnMount: "always",
  })

/** A shared note in the person's own words: trusted. 409 when the team has a live note with the key. */
export const createNote = (input: NoteCreate) => postJson<MemoryEntryView>("/memory", input)

/** A new version; the label stays. 409 when the note changed since `expected_version`. */
export const editNote = (id: string, input: NoteEdit) => putJson<MemoryEntryView>(`/memory/${id}`, input)

/** Hidden from every reader; its versions stay. 409 when it changed since `expectedVersion`. */
export const deleteNote = (id: string, expectedVersion: number) =>
  deleteRequest(`/memory/${id}${query({ expected_version: expectedVersion })}`)

/** The person vouches for the text; tasks it already tainted stay tainted. */
export const markNoteTrusted = (id: string, input: NoteVersion) =>
  postJson<MemoryEntryView>(`/memory/${id}/trust`, input)

/** The person's writes emit no events, so the lists refetch after each one. */
export const refetchNotes = (client: QueryClient) =>
  Promise.all([
    client.invalidateQueries({ queryKey: ["memory"] }),
    client.invalidateQueries({ queryKey: ["memory-versions"] }),
  ])
