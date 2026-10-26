import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import type { RunDetail } from "../src/generated/RunDetail.ts"
import { api } from "./api.ts"
import type { SeedFile } from "./cases.ts"
import type { EventRecorder } from "./events.ts"
import type { ReadyTeam } from "./teams.ts"

export const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
export const message = (error: unknown) => (error instanceof Error ? error.message : String(error))

/** The task once no run of it is running, or the last snapshot when `timeoutMs` passes. */
export async function waitIdle(root: string, timeoutMs = 60_000, pollMs = 300): Promise<RunDetail> {
  const started = Date.now()
  for (;;) {
    const detail = await api.run(root)
    if (detail.runs.length > 0 && detail.runs.every((run) => run.status !== "running")) return detail
    if (Date.now() - started > timeoutMs) return detail
    await sleep(pollMs)
  }
}

/** Cancels the task if any run of it is still running, then waits until none is; a 409 means it already ended. */
export async function stopTask(root: string) {
  const detail = await api.run(root).catch(() => null)
  if (!detail?.runs.some((run) => run.status === "running")) return
  await api.cancel(root).catch(() => undefined)
  await waitIdle(root, 30_000)
}

/** Deletes a team, retrying while the server still sees a task of it running; warns when it gives up. */
export async function removeTeam(id: string, label: string) {
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      await api.deleteTeam(id)
      return
    } catch (error) {
      if (attempt === 2) console.warn(`  couldn't delete team ${label}: ${message(error)}`)
      else await sleep(2000)
    }
  }
}

/** The task's events once the stream has delivered a `run_finished` for every run of the snapshot. */
export async function streamed(recorder: EventRecorder, detail: RunDetail, timeoutMs = 10_000) {
  const started = Date.now()
  const done = () => {
    const finished = new Set(recorder.forTask(detail.root_run_id).filter((e) => e.type === "run_finished").map((e) => e.run_id))
    return detail.runs.every((run) => finished.has(run.id))
  }
  while (!done() && Date.now() - started < timeoutMs) await sleep(200)
  return recorder.forTask(detail.root_run_id)
}

/** A model-free task: the agent makes the scripted calls, then answers "done". */
export async function scripted(teamId: string, agentId: string, scenario: string, task = "scripted") {
  const { run_id } = await api.startRun({ team_id: teamId, agent_id: agentId, task, scenario })
  return run_id
}

/** Writes each file with a scripted save by the team's writer; a clean task stores it trusted. */
export async function seedFiles(team: ReadyTeam, files: SeedFile[]) {
  if (files.length === 0) return
  if (!team.writerId) throw new Error(`team ${team.key} has no agent that can save files to seed`)
  for (const file of files) {
    const args = JSON.stringify({ path: file.path, content: file.content })
    const detail = await waitIdle(await scripted(team.id, team.writerId, `test:call:file_write:${args}`, `Seed ${file.path}`))
    if (!detail.tool_calls.some((c) => c.tool === "file_write" && c.status === "ok")) throw new Error(`seeding ${file.path} failed`)
  }
}

/** Writes one task's raw record to `<dir>/<name>/`, one file per source, for reading afterwards. */
export function writeTaskLog(dir: string, name: string, files: Record<string, unknown>) {
  const folder = join(dir, "tasks", name.replace(/[^\w.-]+/g, "_"))
  mkdirSync(folder, { recursive: true })
  for (const [file, value] of Object.entries(files)) {
    const text = file.endsWith(".jsonl")
      ? (value as unknown[]).map((item) => JSON.stringify(item)).join("\n")
      : JSON.stringify(value, null, 2)
    writeFileSync(join(folder, file), text)
  }
  return folder
}
