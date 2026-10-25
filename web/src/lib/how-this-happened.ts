import type { AgentRecord } from "@/generated/AgentRecord"
import type { ApprovalView } from "@/generated/ApprovalView"
import type { RunDetail } from "@/generated/RunDetail"
import type { RunView } from "@/generated/RunView"
import type { TaintSource } from "@/generated/TaintSource"
import type { ToolCallView } from "@/generated/ToolCallView"
import type { ToolList } from "@/generated/ToolList"
import {
  calledDecision,
  callSummary,
  listText,
  readBeforeText,
  reasonsText,
  RESOLVED,
  sourceLabel,
  toolLabel,
} from "./copy"

/** Search params that open the sheet: `?call=<tool call id>&task=<root run id>`. */
export function sheetSearch(callId: string, rootRunId: string): string {
  return `?${new URLSearchParams({ call: callId, task: rootRunId })}`
}

export type HopKind = "you" | "handoff" | "read" | "rule" | "decided"

export interface Hop {
  kind: HopKind
  label: string
  lines: string[]
  /** A secondary line under the hop's text. */
  note?: string
  /** The agent whose shape marks a handoff hop. */
  agent?: AgentRecord
  /** A decision that stopped the call, so the hop shows a shield instead of a check. */
  stopped?: boolean
  quoted?: boolean
}

export interface Explanation {
  summary: string
  agent?: AgentRecord
  /** "Writer used Save file", without the time. */
  byline: string
  at: string
  hops: Hop[]
  technical: unknown
  rootRunId: string
}

export interface Lookups {
  byId: (id: string) => AgentRecord | undefined
  bySlug: (slug: string) => AgentRecord | undefined
  nameOfSlug: (slug: string) => string
}

/** The top-level run of the turn a run belongs to; its task is what the user typed. */
function turnRun(runs: RunView[], runId: string): RunView | undefined {
  let run = runs.find((r) => r.id === runId)
  while (run?.parent_run_id) {
    const parentId = run.parent_run_id
    run = runs.find((r) => r.id === parentId)
  }
  return run
}

function youAsked(detail: RunDetail | undefined, runId: string): Hop[] {
  const run = detail && turnRun(detail.runs, runId)
  return run ? [{ kind: "you", label: "You asked", lines: [run.task], quoted: true }] : []
}

/** JSON with sorted keys, so two calls with the same arguments compare equal. */
function stableJson(value: unknown): string {
  return JSON.stringify(value, (_key, inner: unknown) =>
    inner && typeof inner === "object" && !Array.isArray(inner)
      ? Object.fromEntries(Object.entries(inner).sort(([a], [b]) => a.localeCompare(b)))
      : inner,
  )
}

const STOPPED = new Set(["refused", "not_offered", "invalid_args", "denied_human", "expired", "cancelled"])

/** Hops for a call that went through an approval card; every line comes from the card. */
export function explainApproval(
  approval: ApprovalView,
  detail: RunDetail | undefined,
  look: Lookups,
): Explanation {
  const path = approval.custody.agent_path
  const agentSlug = path.at(-1)
  const agent = agentSlug ? look.bySlug(agentSlug) : undefined
  const agentName = agentSlug ? look.nameOfSlug(agentSlug) : "An agent"
  const hops: Hop[] = [...youAsked(detail, approval.run_id)]

  const callerSlug = path.at(-2)
  if (callerSlug) {
    const sources = approval.custody.parent_task_sources
    hops.push({
      kind: "handoff",
      label: `${look.nameOfSlug(callerSlug)} asked ${agentName}`,
      lines: [approval.custody.parent_task],
      agent: look.bySlug(callerSlug),
      quoted: true,
    })
    if (sources.length > 0) {
      hops.at(-1)!.note = `This handoff carried outside content: ${listText(sources.map(sourceLabel))}.`
    }
  }

  hops.push({
    kind: "read",
    label: "What it had read before",
    lines: [readBeforeText(approval.custody.taint_sources.map(sourceLabel))],
  })
  // A card means the outcome was Ask, but taint or an off-list host escalate any setting to Ask,
  // so the setting itself is named only when neither is present.
  const escalated = approval.custody.rules.some((rule) => rule.kind === "taint" || rule.kind === "off_allowlist")
  const rules = reasonsText(approval.custody.rules, {
    agent: agentName,
    tool: approval.tool,
    decision: escalated ? null : "ask",
  })
  if (rules.length > 0) hops.push({ kind: "rule", label: "The rule", lines: rules })
  hops.push({
    kind: "decided",
    label: "Who decided",
    lines: [RESOLVED[approval.status](approval)],
    stopped: approval.status !== "approved" && approval.status !== "pending",
  })

  return {
    summary: callSummary(approval.tool, approval.action, look.nameOfSlug),
    agent,
    byline: `${agentName} used ${toolLabel(approval.tool)}`,
    at: approval.created_at,
    hops,
    technical: { preview: approval.preview, action: approval.action, custody: approval.custody },
    rootRunId: approval.root_run_id,
  }
}

/** The task's taint source an earlier call read, when its target names that source exactly. */
function taintedBy(call: ToolCallView, sources: TaintSource[]): TaintSource | undefined {
  const target = call.action?.target
  if (!target) return undefined
  return sources.find(
    (source) =>
      (source.kind === "inbox" && target.kind === "message" && target.id === source.message_id) ||
      (source.kind === "web" && target.kind === "host" && target.url === source.url) ||
      (source.kind === "file" && target.kind === "file" && target.path === source.path) ||
      (source.kind === "note" && target.kind === "note" && target.scope === source.scope && target.key === source.key),
  )
}

/** Hops for a call with no card, from the task's runs and its earlier calls. */
export function explainCall(call: ToolCallView, detail: RunDetail, tools: ToolList | undefined, look: Lookups): Explanation {
  const agent = look.byId(call.agent_id)
  const agentName = agent?.name ?? "An agent"
  const hops: Hop[] = [...youAsked(detail, call.run_id)]
  const index = detail.tool_calls.findIndex((c) => c.id === call.id)
  const earlierCalls = detail.tool_calls.slice(0, Math.max(index, 0))

  const args = call.args as Record<string, unknown> | null
  if (call.tool === "delegate" && call.action?.target.kind === "agent" && typeof args?.task === "string") {
    hops.push({
      kind: "handoff",
      label: `${agentName} handed ${look.nameOfSlug(call.action.target.slug)} this step`,
      lines: [args.task],
      agent,
      quoted: true,
    })
  } else {
    const run = detail.runs.find((r) => r.id === call.run_id)
    const parent = run?.parent_run_id ? detail.runs.find((r) => r.id === run.parent_run_id) : undefined
    if (run && parent) {
      const caller = look.byId(parent.agent_id)
      hops.push({
        kind: "handoff",
        label: `${caller?.name ?? "Its caller"} asked ${agentName}`,
        lines: [run.task],
        note:
          run.task_sources.length > 0
            ? `This handoff carried outside content: ${listText(run.task_sources.map(sourceLabel))}.`
            : undefined,
        agent: caller,
        quoted: true,
      })
    }
  }

  // Only reads that tainted the task count; a tool that can taint (ToolInfo.taints) did not
  // necessarily do so, and a trusted file read never does.
  const earlierOk = earlierCalls.filter((c) => c.status === "ok")
  const read = earlierOk
    .map((c) => taintedBy(c, detail.taint_sources))
    .filter((source): source is TaintSource => source !== undefined)
  if (read.length > 0) {
    hops.push({ kind: "read", label: "What it had read before", lines: [readBeforeText(read.map(sourceLabel))] })
  } else if (tools) {
    // A read that can taint without naming one source (the inbox list) leaves the answer
    // unknown, so "nothing" is claimed only when no such read came first.
    const taints = new Set(tools.tools.filter((t) => t.taints).map((t) => t.name as string))
    if (!earlierOk.some((c) => taints.has(c.tool))) {
      hops.push({ kind: "read", label: "What it had read before", lines: [readBeforeText([])] })
    }
  }

  const rules = reasonsText(call.rules, { agent: agentName, tool: call.tool, decision: call.decision })
  if (rules.length > 0) hops.push({ kind: "rule", label: "The rule", lines: rules })

  const json = stableJson(call.args)
  const earlier =
    earlierCalls.findLast(
      (c) =>
        c.tool === call.tool &&
        stableJson(c.args) === json &&
        (c.status === "refused" || c.status === "denied_human" || c.status === "expired"),
    ) ?? null
  const decided = calledDecision(call, earlier)
  if (decided) {
    hops.push({
      kind: "decided",
      label: "Who decided",
      lines: [decided],
      stopped: call.status !== null && STOPPED.has(call.status),
    })
  }

  return {
    summary: callSummary(call.tool, call.action, look.nameOfSlug),
    agent,
    byline: `${agentName} used ${toolLabel(call.tool)}`,
    at: call.requested_at,
    hops,
    technical: {
      args: call.args,
      action: call.action,
      rules: call.rules,
      status: call.status,
      duration_ms: call.duration_ms,
    },
    rootRunId: call.root_run_id,
  }
}
