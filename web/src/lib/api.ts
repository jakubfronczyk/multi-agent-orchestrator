import type { ErrorBody } from "@/generated/ErrorBody"
import type { ErrorCode } from "@/generated/ErrorCode"

/** A non-2xx answer; `field` names the input the server rejected, when it names one. */
export class ApiError extends Error {
  readonly status: number
  readonly code: ErrorCode | null
  readonly field: string | null

  constructor(status: number, body: ErrorBody | null) {
    super(body?.error.message ?? `Request failed with status ${status}`)
    this.name = "ApiError"
    this.status = status
    this.code = body?.error.code ?? null
    this.field = body?.error.field ?? null
  }
}

export function isApiError(error: unknown, status?: number): error is ApiError {
  return error instanceof ApiError && (status === undefined || error.status === status)
}

async function send(path: string, init?: RequestInit): Promise<Response> {
  const response = await fetch(`/api${path}`, init)
  if (response.ok) return response
  // A proxy error page or an empty body is not an ErrorBody; the status alone still identifies it.
  const body = (await response.json().catch(() => null)) as ErrorBody | null
  throw new ApiError(response.status, body?.error ? body : null)
}

export async function getJson<T>(path: string): Promise<T> {
  return (await send(path)).json() as Promise<T>
}

export async function postJson<T>(path: string, body?: unknown): Promise<T> {
  const response = await send(path, {
    method: "POST",
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  return (response.status === 204 ? undefined : response.json()) as Promise<T>
}

export async function putJson<T>(path: string, body: unknown): Promise<T> {
  const response = await send(path, {
    method: "PUT",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  })
  return response.json() as Promise<T>
}

export async function postForm<T>(path: string, form: FormData): Promise<T> {
  return (await send(path, { method: "POST", body: form })).json() as Promise<T>
}

export async function deleteRequest(path: string): Promise<void> {
  await send(path, { method: "DELETE" })
}

/** Builds a query string from the set values only. */
export function query(params: Record<string, string | number | undefined | null>): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") search.set(key, String(value))
  }
  const text = search.toString()
  return text ? `?${text}` : ""
}
