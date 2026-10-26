import { execFileSync } from "node:child_process"
import type { AgentInput } from "../src/generated/AgentInput.ts"
import type { RunDetail } from "../src/generated/RunDetail.ts"
import type { ToolCallView } from "../src/generated/ToolCallView.ts"
import type { ToolName } from "../src/generated/ToolName.ts"

export const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))
export const message = (error: unknown) => (error instanceof Error ? error.message : String(error))

/** A sortable, filename-safe timestamp, such as `2026-10-26T08-58-06`. */
export const stamp = () => new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)

/** The server caps team names at 60 characters. */
export const teamName = (...parts: (string | number)[]) => parts.join(" ").slice(0, 60)

/** The tool is offered and not set to Deny. */
export const allows = (tools: AgentInput["tools"], tool: ToolName) => tools[tool] !== undefined && tools[tool] !== "deny"

/** The Lead's answers in a task, one per turn. */
export const answerText = (detail: RunDetail | null | undefined, separator = "\n\n") =>
  (detail?.turns ?? []).map((turn) => turn.summary ?? "").join(separator)

/** The text a `file_write` call saves. */
export const contentOf = (call: ToolCallView) => String((call.args as { content?: unknown } | null)?.content ?? "")

/** The commit the harness ran from, marked when `web/eval/` had uncommitted changes, so a report names its harness. */
export function harnessVersion(): string {
  try {
    const git = (...args: string[]) => execFileSync("git", args, { cwd: import.meta.dirname, encoding: "utf8" }).trim()
    const dirty = git("status", "--porcelain", "--", ".") !== ""
    return `${git("rev-parse", "--short", "HEAD")}${dirty ? " with uncommitted eval changes" : ""}`
  } catch {
    return "unknown"
  }
}
