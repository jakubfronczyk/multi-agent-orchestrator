import type { Action } from "@/generated/Action"
import type { ApprovalView } from "@/generated/ApprovalView"
import type { CallStatus } from "@/generated/CallStatus"
import type { Decision } from "@/generated/Decision"
import type { Rule } from "@/generated/Rule"
import type { TaintSource } from "@/generated/TaintSource"
import type { ToolCallView } from "@/generated/ToolCallView"
import type { ToolName } from "@/generated/ToolName"
import { clockTime } from "./time"

// Every user-facing sentence built from API data lives here. Each line states only what its
// fields say: what a task read is task-level, never a claim that it flowed into one call.

export const TOOL_LABEL: Record<ToolName, string> = {
  delegate: "Hand off",
  plan: "Plan",
  memory_read: "Read notes",
  memory_search: "Search notes",
  memory_write: "Write notes",
  file_list: "List files",
  file_read: "Read file",
  file_write: "Save file",
  inbox_list: "List inbox",
  inbox_read: "Read inbox",
  http_fetch: "Open website",
  csv_query: "Query table",
  send_message: "Send message",
}

/** The tool's label; a name the model made up is shown as sent. */
export function toolLabel(tool: string): string {
  return tool in TOOL_LABEL ? TOOL_LABEL[tool as ToolName] : `"${tool}"`
}

export const DECISION_LABEL: Record<Decision, string> = {
  allow: "Allow",
  ask: "Ask me",
  deny: "Deny",
}

/** One line for what a call does, from its tool and action target. */
export function callSummary(tool: string, action: Action | null, agentName: (slug: string) => string): string {
  if (!action) return tool in TOOL_LABEL ? toolLabel(tool) : `Unknown tool "${tool}"`
  const target = action.target
  switch (target.kind) {
    case "file":
      if (tool === "file_write") return `Save ${target.path}`
      if (tool === "file_list") return `List files in ${target.path}`
      if (tool === "csv_query") return `Query ${target.path}`
      return `Read ${target.path}`
    case "recipient":
      return target.channel === "email"
        ? `Email ${target.address}`
        : `Message ${target.address} on ${target.channel}`
    case "message":
      return `Read inbox message ${target.id}`
    case "host":
      return `Open ${target.url}`
    case "note":
      return `${tool === "memory_write" ? "Save" : "Read"} ${target.scope} note ${target.key}`
    case "query":
      if (tool === "inbox_list") return "List the inbox"
      if (tool === "memory_search") return `Search notes for "${target.text}"`
      return `${toolLabel(tool)}: ${target.text}`
    case "agent":
      return `Hand ${agentName(target.slug)} a step`
    case "plan":
      return `Plan ${target.steps.length} ${target.steps.length === 1 ? "step" : "steps"}`
    default:
      return unreachable(target)
  }
}

export function sourceLabel(source: TaintSource): string {
  switch (source.kind) {
    case "inbox":
      return `inbox message ${source.message_id}`
    case "web":
      return source.url
    case "note":
      return `${source.scope} note ${source.key}`
    case "file":
      return `file ${source.path}`
    default:
      return unreachable(source)
  }
}

/** "a", "a and b", "a, b and c". */
export function listText(items: string[]): string {
  if (items.length <= 1) return items.join("")
  return `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`
}

/** Task-level: what the task had read from outside, never what one call used. */
export function readBeforeText(labels: string[]): string {
  const unique = [...new Set(labels)]
  return unique.length === 0 ? "Nothing from outside." : `This task read ${listText(unique)}.`
}

export interface RuleContext {
  agent: string
  tool: string
  decision: Decision | null
}

/** One plain-words line per rule kind; a new kind fails the build at `unreachable`. */
export function ruleText(rule: Rule, ctx: RuleContext): string {
  const tool = toolLabel(ctx.tool)
  switch (rule.kind) {
    case "agent_policy":
      return ctx.decision
        ? `${ctx.agent}'s "${tool}" is set to ${DECISION_LABEL[ctx.decision]}.`
        : `${ctx.agent}'s setting for "${tool}" decided this.`
    case "off_allowlist":
      return `${rule.host} isn't on your allowed sites.`
    case "taint":
      return `This task read ${sourceLabel(rule.source)}, so from then on every save, send and website fetch asks you, whatever the settings.`
    case "not_offered":
      return `${ctx.agent} doesn't have ${tool}, so the request was refused.`
    case "invalid_args":
      return "The request was malformed, so it was refused."
    case "repeat_of_refused":
      return "It repeats a request already refused in this task, so it was stopped without asking."
    case "external_effect":
      return `${tool} reaches people outside, so it can only be Ask me or Deny.`
    case "draft_only":
      return "This turn is draft only: saving, notes, sending and opening websites are refused without asking you."
    default:
      return unreachable(rule)
  }
}

/** The rule lines for a call; no rules means its own setting let it run. */
export function reasonsText(rules: Rule[], ctx: RuleContext): string[] {
  if (rules.length > 0) return rules.map((rule) => ruleText(rule, ctx))
  return ctx.decision === "allow" ? [`${ctx.agent}'s "${toolLabel(ctx.tool)}" is set to Allow.`] : []
}

const minutesBetween = (from: string, to: string) =>
  Math.round((Date.parse(to) - Date.parse(from)) / 60_000)

/** Who decided an approval card, by its status. */
export const RESOLVED: Record<ApprovalView["status"], (approval: ApprovalView) => string> = {
  pending: (a) =>
    `Waiting for you. If nobody answers in ${minutesBetween(a.created_at, a.expires_at)} minutes, it counts as no.`,
  approved: (a) => (a.resolved_at ? `You said yes at ${clockTime(a.resolved_at)}.` : "You said yes."),
  denied: (a) =>
    `You said no${a.resolved_at ? ` at ${clockTime(a.resolved_at)}` : ""}${a.note ? `: "${a.note}"` : ""}.`,
  expired: (a) =>
    `Nobody answered in ${minutesBetween(a.created_at, a.expires_at)} minutes, so it counted as no.`,
  cancelled: () => "The card was closed before anyone answered: the task stopped first.",
}

/**
 * Who decided a call that had no card. `earlier` is the matching call this one repeats, when
 * the task has one; without it the repeat line names no decider.
 */
export function calledDecision(call: ToolCallView, earlier: ToolCallView | null): string | null {
  if (call.status === null) {
    return call.phase === "running" ? "On its own. It is still running." : null
  }
  return statusDecision(call.status, call, earlier)
}

function statusDecision(status: CallStatus, call: ToolCallView, earlier: ToolCallView | null): string {
  switch (status) {
    case "ok":
      return "On its own, under the rule above."
    case "refused":
      if (call.rules.some((rule) => rule.kind === "repeat_of_refused")) {
        if (earlier?.status === "denied_human" || earlier?.status === "expired") {
          return "Stopped automatically. You already said no to this exact request."
        }
        if (earlier?.status === "refused") {
          return "Stopped automatically. Your rules already refused this exact request."
        }
        return "Stopped automatically, without asking anyone."
      }
      return "Your rules. Nobody was asked, because no answer could allow it."
    case "not_offered":
      return "Your rules. Nobody was asked."
    case "invalid_args":
      return "Refused automatically. Nobody was asked."
    case "denied_human":
      return "You said no."
    case "expired":
      return "Nobody answered in time, so it counted as no."
    case "timeout":
      return "On its own. It then ran out of time."
    case "failed":
    case "internal":
      return "On its own. It then didn't work."
    case "cancelled":
      return "The task was cancelled first."
    case "child_failed":
      return "On its own. The teammate's work didn't finish."
    case "child_cancelled":
      return "On its own. The teammate's work was cancelled."
    case "interrupted":
      return "Stopped by a restart."
    default:
      return unreachable(status)
  }
}

function unreachable(value: never): never {
  throw new Error(`Unhandled value: ${JSON.stringify(value)}`)
}
