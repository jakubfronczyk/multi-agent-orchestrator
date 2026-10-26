import type { AgentInput } from "../src/generated/AgentInput.ts"
import type { AgentRecord } from "../src/generated/AgentRecord.ts"
import type { AnswerApprovalRequest } from "../src/generated/AnswerApprovalRequest.ts"
import type { ApprovalList } from "../src/generated/ApprovalList.ts"
import type { FileAccess } from "../src/generated/FileAccess.ts"
import type { FileView } from "../src/generated/FileView.ts"
import type { Health } from "../src/generated/Health.ts"
import type { OutboxList } from "../src/generated/OutboxList.ts"
import type { RunDetail } from "../src/generated/RunDetail.ts"
import type { StartRunRequest } from "../src/generated/StartRunRequest.ts"
import type { StartRunResponse } from "../src/generated/StartRunResponse.ts"
import type { TeamList } from "../src/generated/TeamList.ts"
import type { TeamView } from "../src/generated/TeamView.ts"
import type { UploadResponse } from "../src/generated/UploadResponse.ts"

export const BASE = process.env.EVAL_API ?? "http://localhost:8787/api"

async function call<T>(method: string, path: string, payload?: unknown): Promise<T> {
  const response = await fetch(`${BASE}${path}`, {
    method,
    headers: payload === undefined ? undefined : { "content-type": "application/json" },
    body: payload === undefined ? undefined : JSON.stringify(payload),
  })
  const text = await response.text()
  if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${text}`)
  return (text && response.headers.get("content-type")?.includes("application/json") ? JSON.parse(text) : undefined) as T
}

export const api = {
  health: () => call<Health>("GET", "/health"),
  teams: () => call<TeamList>("GET", "/teams"),
  createTeam: (name: string) => call<TeamView>("POST", "/teams", { name }),
  deleteTeam: (id: string) => call<void>("DELETE", `/teams/${id}`),
  setFileAccess: (team: string, of: string, access: FileAccess) =>
    call<void>("PUT", `/teams/${team}/file-access/${of}`, { access }),
  agents: () => call<AgentRecord[]>("GET", "/agents"),
  createAgent: (input: AgentInput) => call<AgentRecord>("POST", "/agents", input),
  replaceAgent: (id: string, input: AgentInput) => call<AgentRecord>("PUT", `/agents/${id}`, input),
  startRun: (request: StartRunRequest) => call<StartRunResponse>("POST", "/runs", request),
  /** With `scenario` the follow-up plays scripted replies, so no model is called. */
  followUp: (root: string, text: string, scenario?: string) =>
    call<StartRunResponse>("POST", `/runs/${root}/messages`, scenario ? { text, scenario } : { text }),
  cancel: (root: string) => call<void>("POST", `/runs/${root}/cancel`),
  run: (root: string) => call<RunDetail>("GET", `/runs/${root}`),
  approvals: () => call<ApprovalList>("GET", "/approvals"),
  answer: (id: string, answer: AnswerApprovalRequest) => call<void>("POST", `/approvals/${id}`, answer),
  outbox: (team: string) => call<OutboxList>("GET", `/outbox?team_id=${team}`),
  upload: async (team: string, name: string, content: string): Promise<UploadResponse> => {
    const form = new FormData()
    form.append("file", new Blob([content], { type: "text/plain" }), name)
    const response = await fetch(`${BASE}/files?team_id=${team}`, { method: "POST", body: form })
    if (!response.ok) throw new Error(`POST /files: ${response.status} ${await response.text()}`)
    return (await response.json()) as UploadResponse
  },
  file: (team: string, path: string) =>
    call<FileView>("GET", `/files?team_id=${team}&path=${encodeURIComponent(path)}`),
}
