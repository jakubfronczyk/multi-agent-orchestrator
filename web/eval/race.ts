// Scripted checks of tools, cross-team file access, races and the event stream. Every agent plays
// fixed calls (`test:*` scenarios), so no model is called and every run gives the same story.
//   node eval/race.ts [--only name,name] [--loops 5]

import { join } from "node:path"
import { mkdirSync, writeFileSync } from "node:fs"
import { parseArgs } from "node:util"
import type { FileView } from "../src/generated/FileView.ts"
import type { RunDetail } from "../src/generated/RunDetail.ts"
import type { ToolCallView } from "../src/generated/ToolCallView.ts"
import { api } from "./api.ts"
import { EventRecorder } from "./events.ts"
import { grade, type Check, type Transcript } from "./graders.ts"
import { message, scripted, seedFiles, sleep, streamed, waitIdle, writeTaskLog } from "./setup.ts"
import { createTeam, loadSeeded, TEAMS, type ReadyTeam } from "./teams.ts"

const { values } = parseArgs({ options: { only: { type: "string" }, loops: { type: "string", default: "5" } } })
const LOOPS = Number(values.loops)

interface Ctx {
  a: ReadyTeam
  b: ReadyTeam
  aLead: string
  recorder: EventRecorder
  dir: string
  /** Every task a test started, for the orchestration check and the replay check. */
  roots: string[]
}

const call = (...parts: [string, unknown][]) => parts.map(([tool, args]) => `${tool}:${JSON.stringify(args)}`).join("|")
const seq = (...parts: [string, unknown][]) => `test:seq:${call(...parts)}`
const one = (tool: string, args: unknown) => `test:call:${tool}:${JSON.stringify(args)}`
const INBOX = ["inbox_read", { id: "access-request-1042" }] as [string, unknown]

async function start(ctx: Ctx, team: ReadyTeam, scenario: string, task: string) {
  const root = await scripted(team.id, team.writerId as string, scenario, task)
  ctx.roots.push(root)
  return root
}

/** The first pending card of the task, once it appears. */
async function card(root: string, timeoutMs = 20_000) {
  const started = Date.now()
  for (;;) {
    const detail = await api.run(root)
    if (detail.pending_approvals[0]) return detail.pending_approvals[0]
    if (Date.now() - started > timeoutMs) throw new Error(`no card in task ${root}`)
    await sleep(150)
  }
}

const file = async (team: ReadyTeam, path: string): Promise<Extract<FileView, { kind: "file" }> | null> => {
  try {
    const view = await api.file(team.id, path)
    return view.kind === "file" ? view : null
  } catch {
    return null
  }
}

const callsOf = (detail: RunDetail, tool: string) => detail.tool_calls.filter((c) => c.tool === tool)
const rules = (c: ToolCallView | undefined) => (c?.rules ?? []).map((r) => r.kind).join(",")
const expect = (problems: string[], ok: boolean, text: string) => {
  if (!ok) problems.push(text)
}

type Test = (ctx: Ctx) => Promise<string[]>

const TESTS: Record<string, Test> = {
  /** Every data tool works in the team's own folder, and a clean task raises no card. */
  async "own-folder-tools"(ctx) {
    const p: string[] = []
    const detail = await waitIdle(
      await start(
        ctx,
        ctx.a,
        seq(
          ["file_write", { path: "notes/a.md", content: "alpha" }],
          ["file_read", { path: "notes/a.md" }],
          ["file_list", { path: "notes" }],
          ["csv_query", { path: "data/systems.csv", count: true }],
          ["memory_write", { scope: "shared", key: "race.alpha", content: "alpha note" }],
          ["memory_read", { scope: "shared", key: "race.alpha" }],
        ),
        "own folder tools",
      ),
    )
    for (const c of detail.tool_calls) expect(p, c.status === "ok", `${c.tool} ended ${c.status} (${rules(c)})`)
    expect(p, detail.tool_calls.length === 6, `${detail.tool_calls.length} calls, expected 6`)
    expect(p, detail.taint_sources.length === 0, "a clean task got tainted")
    expect(p, (await api.approvals()).approvals.every((a) => a.root_run_id !== detail.root_run_id), "a clean task raised a card")
    expect(p, (await file(ctx.a, "notes/a.md"))?.trust === "trusted", "notes/a.md is not trusted")
    return p
  },

  /** `data/` is read-only for every team. */
  async "data-read-only"(ctx) {
    const detail = await waitIdle(await start(ctx, ctx.a, one("file_write", { path: "data/hack.md", content: "x" }), "write data/"))
    const write = callsOf(detail, "file_write")[0]
    return write?.status === "ok" ? ["a write into data/ went through"] : []
  },

  /** After outside content the save asks, runs once approved, and is stored untrusted. */
  async "taint-raises-write"(ctx) {
    const p: string[] = []
    const root = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path: "tainted/x.md", content: "from inbox" }]), "taint raises write")
    const pending = await card(root)
    expect(p, pending.tool === "file_write", `card is for ${pending.tool}`)
    await api.answer(pending.id, { decision: "approve" })
    const detail = await waitIdle(root)
    expect(p, callsOf(detail, "file_write")[0]?.status === "ok", "approved save did not run")
    expect(p, detail.taint_sources.some((s) => s.kind === "inbox"), "task not tainted by the inbox")
    expect(p, (await file(ctx.a, "tainted/x.md"))?.trust === "untrusted", "tainted save stored trusted")
    return p
  },

  /** Two clean saves to one path at once: the file holds one of them, labelled with its writer. */
  async "concurrent-clean-writes"(ctx) {
    const p: string[] = []
    for (let i = 0; i < LOOPS; i++) {
      const path = `race/same-${i}.md`
      const [x, y] = await Promise.all(
        ["X", "Y"].map((who) => start(ctx, ctx.a, one("file_write", { path, content: `${who}-${i}` }), `race ${who} ${i}`)),
      )
      await Promise.all([waitIdle(x), waitIdle(y)])
      const view = await file(ctx.a, path)
      const winner = view?.content === `X-${i}` ? x : view?.content === `Y-${i}` ? y : null
      expect(p, winner !== null, `${path} holds "${view?.content}", neither write`)
      expect(p, winner === null || view?.written_by_root_run_id === winner, `${path}: content from one task, writer recorded as another`)
    }
    return p
  },

  /** A clean and a tainted save to one path at once: the label always matches whose content won. */
  async "clean-vs-tainted-write"(ctx) {
    const p: string[] = []
    for (let i = 0; i < LOOPS; i++) {
      const path = `race/mixed-${i}.md`
      const tainted = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path, content: `TAINTED-${i}` }]), `tainted ${i}`)
      const pending = await card(tainted)
      const [clean] = await Promise.all([
        start(ctx, ctx.a, one("file_write", { path, content: `CLEAN-${i}` }), `clean ${i}`),
        api.answer(pending.id, { decision: "approve" }),
      ])
      await Promise.all([waitIdle(clean), waitIdle(tainted)])
      const view = await file(ctx.a, path)
      if (view?.content === `CLEAN-${i}`) {
        expect(p, view.trust === "trusted" && view.written_by_root_run_id === clean, `${path}: clean content but ${view.trust}, writer ${view.written_by_root_run_id}`)
      } else if (view?.content === `TAINTED-${i}`) {
        expect(p, view.trust === "untrusted" && view.written_by_root_run_id === tainted, `${path}: tainted content but ${view.trust}, writer ${view.written_by_root_run_id}`)
      } else p.push(`${path} holds "${view?.content}"`)
    }
    return p
  },

  /** Approve and deny the same card at once, each sent first on alternate loops: exactly one answer wins and the call follows it. */
  async "double-answer"(ctx) {
    const p: string[] = []
    const winners: string[] = []
    for (let i = 0; i < LOOPS; i++) {
      const root = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path: `race/double-${i}.md`, content: "x" }]), `double ${i}`)
      const pending = await card(root)
      const order: ("approve" | "deny")[] = i % 2 === 0 ? ["approve", "deny"] : ["deny", "approve"]
      const results = await Promise.allSettled(order.map((decision) => api.answer(pending.id, { decision })))
      const accepted = order.filter((_, k) => results[k]?.status === "fulfilled")
      expect(p, accepted.length === 1, `loop ${i}: ${accepted.length} answers accepted`)
      const detail = await waitIdle(root)
      const status = callsOf(detail, "file_write")[0]?.status
      const won = accepted[0]
      if (won) winners.push(won)
      const expected = won === "approve" ? "ok" : "denied_human"
      expect(p, !won || status === expected, `loop ${i}: ${won} won but the call ended ${status}`)
    }
    // Both outcomes must be seen, or the check only ever exercised one side of the race.
    if (LOOPS > 1) {
      expect(p, winners.includes("approve") && winners.includes("deny"), `only ${[...new Set(winners)].join(",") || "nothing"} ever won`)
    }
    return p
  },

  /** A save whose path differs from a trusted file only in letter case is refused, so tainted bytes never sit behind the trusted label. */
  async "case-variant-write"(ctx) {
    const p: string[] = []
    await waitIdle(await start(ctx, ctx.a, one("file_write", { path: "race/case.md", content: "CLEAN" }), "case: clean save"))
    const tainted = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path: "RACE/Case.md", content: "TAINTED" }]), "case: tainted twin")
    const pending = await card(tainted)
    await api.answer(pending.id, { decision: "approve" })
    const detail = await waitIdle(tainted)
    const write = callsOf(detail, "file_write")[0]
    expect(p, write?.status === "failed", `the case twin's save ended ${write?.status}`)
    const view = await file(ctx.a, "race/case.md")
    expect(p, view?.content === "CLEAN" && view.trust === "trusted", `race/case.md holds "${view?.content}" labelled ${view?.trust}`)
    return p
  },

  /** Cancelling a task with a card waiting cancels the card and the call. */
  async "cancel-pending-card"(ctx) {
    const p: string[] = []
    const root = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path: "race/cancel.md", content: "x" }]), "cancel pending card")
    const pending = await card(root)
    await api.cancel(root)
    const detail = await waitIdle(root)
    const approval = (await api.approvals()).approvals.find((a) => a.id === pending.id)
    expect(p, approval?.status === "cancelled", `card ended ${approval?.status}`)
    expect(p, callsOf(detail, "file_write")[0]?.status === "cancelled", `call ended ${callsOf(detail, "file_write")[0]?.status}`)
    expect(p, detail.runs[0]?.status === "cancelled", `task ended ${detail.runs[0]?.status}`)
    expect(p, (await file(ctx.a, "race/cancel.md")) === null, "the cancelled save was written")
    try {
      await api.answer(pending.id, { decision: "approve" })
      p.push("answering a cancelled card was accepted")
    } catch {
      // 409 expected.
    }
    return p
  },

  /** A running task refuses a follow-up and blocks deleting its team; cancel ends it. */
  async "running-task-guards"(ctx) {
    const p: string[] = []
    const { run_id: root } = await api.startRun({ team_id: ctx.a.id, agent_id: ctx.aLead, task: "hang", scenario: "test:hang" })
    ctx.roots.push(root)
    await sleep(500)
    await api.followUp(root, "more").then(
      () => p.push("follow-up accepted while running"),
      (error: unknown) => expect(p, message(error).includes(": 409 "), `follow-up: ${message(error)}`),
    )
    await api.deleteTeam(ctx.a.id).then(
      () => p.push("deleted a team with a running task"),
      (error: unknown) => expect(p, message(error).includes(": 409 "), `delete: ${message(error)}`),
    )
    await api.cancel(root)
    const detail = await waitIdle(root, 15_000)
    expect(p, detail.runs[0]?.status === "cancelled", `task ended ${detail.runs[0]?.status}`)
    return p
  },

  /** Without a grant, another team's folder is refused before any card, and taints nothing. */
  async "cross-no-grant"(ctx) {
    const p: string[] = []
    const detail = await waitIdle(await start(ctx, ctx.a, one("file_read", { path: `teams/${ctx.b.slug}/shared.md` }), "cross no grant"))
    const read = callsOf(detail, "file_read")[0]
    expect(p, read?.status !== "ok", "read another team's folder without a grant")
    expect(p, rules(read).includes("no_access"), `refused for ${rules(read) || "no rule"}, not no_access`)
    expect(p, detail.taint_sources.length === 0, "a refused read tainted the task")
    expect(p, (await api.approvals()).approvals.every((a) => a.root_run_id !== detail.root_run_id), "a refused read raised a card")
    return p
  },

  /** A read grant lets reads through and still refuses writes. */
  async "cross-read-grant"(ctx) {
    const p: string[] = []
    await api.setFileAccess(ctx.a.id, ctx.b.id, "read")
    const detail = await waitIdle(
      await start(
        ctx,
        ctx.a,
        seq(["file_read", { path: `teams/${ctx.b.slug}/shared.md` }], ["file_write", { path: `teams/${ctx.b.slug}/from-a.md`, content: "x" }]),
        "cross read grant",
      ),
    )
    const [read] = callsOf(detail, "file_read")
    const [write] = callsOf(detail, "file_write")
    expect(p, read?.status === "ok", `granted read ended ${read?.status} (${rules(read)})`)
    expect(p, write?.status !== "ok", "wrote into a read-only granted folder")
    expect(p, (await file(ctx.b, "from-a.md")) === null, "from-a.md exists in B's folder")
    return p
  },

  /** A read-write grant lets a clean write through, labelled trusted and with the writing team. */
  async "cross-write-grant"(ctx) {
    const p: string[] = []
    await api.setFileAccess(ctx.a.id, ctx.b.id, "read_write")
    const detail = await waitIdle(await start(ctx, ctx.a, one("file_write", { path: `teams/${ctx.b.slug}/granted.md`, content: "from A" }), "cross write grant"))
    expect(p, callsOf(detail, "file_write")[0]?.status === "ok", `granted write ended ${callsOf(detail, "file_write")[0]?.status}`)
    const view = await file(ctx.b, "granted.md")
    expect(p, view?.content === "from A", "B does not see A's write at granted.md")
    expect(p, view?.trust === "trusted", `labelled ${view?.trust}`)
    expect(p, view?.written_by_team_id === ctx.a.id, "writer team is not A")
    return p
  },

  /** A tainted task's write into a granted folder still asks, and is stored untrusted. */
  async "cross-tainted-write"(ctx) {
    const p: string[] = []
    await api.setFileAccess(ctx.a.id, ctx.b.id, "read_write")
    const root = await start(ctx, ctx.a, seq(INBOX, ["file_write", { path: `teams/${ctx.b.slug}/tainted.md`, content: "x" }]), "cross tainted write")
    const pending = await card(root)
    await api.answer(pending.id, { decision: "approve" })
    await waitIdle(root)
    expect(p, (await file(ctx.b, "tainted.md"))?.trust === "untrusted", "tainted cross-team write stored trusted")
    return p
  },

  /** Reading another team's untrusted file taints the reader: the label crosses with the data. */
  async "cross-label-travels"(ctx) {
    const p: string[] = []
    const seed = await start(ctx, ctx.b, seq(INBOX, ["file_write", { path: "untrusted.md", content: "outside" }]), "B untrusted file")
    await api.answer((await card(seed)).id, { decision: "approve" })
    await waitIdle(seed)
    await api.setFileAccess(ctx.a.id, ctx.b.id, "read")
    const detail = await waitIdle(await start(ctx, ctx.a, one("file_read", { path: `teams/${ctx.b.slug}/untrusted.md` }), "cross label"))
    expect(p, callsOf(detail, "file_read")[0]?.status === "ok", "read of granted untrusted file failed")
    expect(p, detail.taint_sources.some((s) => s.kind === "file"), "reading an untrusted file did not taint the task")
    return p
  },

  /** A grant revoked while a task waits on a card applies at the task's next file call. */
  async "revoke-mid-task"(ctx) {
    const p: string[] = []
    await api.setFileAccess(ctx.a.id, ctx.b.id, "read")
    const path = `teams/${ctx.b.slug}/shared.md`
    const root = await start(
      ctx,
      ctx.a,
      seq(["file_read", { path }], INBOX, ["file_write", { path: "race/pause.md", content: "x" }], ["file_list", { path: `teams/${ctx.b.slug}` }]),
      "revoke mid task",
    )
    const pending = await card(root)
    await api.setFileAccess(ctx.a.id, ctx.b.id, "none")
    await api.answer(pending.id, { decision: "approve" })
    const detail = await waitIdle(root)
    expect(p, callsOf(detail, "file_read")[0]?.status === "ok", "the read before the revoke failed")
    const after = callsOf(detail, "file_list")[0]
    expect(p, after?.status !== "ok", "a call after the revoke still reached B's folder")
    expect(p, rules(after).includes("no_access"), `post-revoke call refused for ${rules(after) || "no rule"}`)
    return p
  },

  /** Shared notes belong to one team: another team reading the same key gets nothing. */
  async "notes-per-team"(ctx) {
    const p: string[] = []
    await waitIdle(await start(ctx, ctx.a, one("memory_write", { scope: "shared", key: "race.secret", content: "only-team-a" }), "A note"))
    const detail = await waitIdle(await start(ctx, ctx.b, one("memory_read", { scope: "shared", key: "race.secret" }), "B reads A's note"))
    const read = callsOf(detail, "memory_read")[0]
    expect(p, !(read?.result_preview ?? "").includes("only-team-a"), "team B read team A's shared note")
    return p
  },
}

/** Replaying the log from before the first task gives exactly the events the live stream delivered. */
async function replayCheck(ctx: Ctx): Promise<string[]> {
  const live = ctx.recorder.events.filter((e) => ctx.roots.includes(e.root_run_id))
  if (live.length === 0) return ["no live events recorded"]
  const first = Math.min(...live.map((e) => e.id))
  const last = Math.max(...live.map((e) => e.id))
  const replay = new EventRecorder(first - 1)
  const started = Date.now()
  while (!replay.events.some((e) => e.id >= last) && Date.now() - started < 15_000) await sleep(200)
  replay.stop()
  const ours = replay.events.filter((e) => ctx.roots.includes(e.root_run_id))
  const p: string[] = []
  const liveIds = live.map((e) => e.id).join(",")
  const replayIds = ours.map((e) => e.id).join(",")
  expect(p, liveIds === replayIds, `live and replay differ: ${live.length} live, ${ours.length} replayed`)
  const byId = new Map(ours.map((e) => [e.id, JSON.stringify(e)]))
  const changed = live.filter((e) => byId.has(e.id) && byId.get(e.id) !== JSON.stringify(e)).map((e) => e.id)
  expect(p, changed.length === 0, `events changed between live and replay: ${changed.slice(0, 5).join(",")}`)
  return p
}

async function main() {
  const health = await api.health()
  console.log(`server up, provider ${health.provider}; scripted tasks only`)
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)
  const dir = join(import.meta.dirname, "out", `race-${stamp}`)
  mkdirSync(dir, { recursive: true })
  const seeded = await loadSeeded()
  const plan = TEAMS.find((t) => t.key === "do-it-all")
  if (!plan) throw new Error("no do-it-all team plan")
  const recorder = new EventRecorder()
  await recorder.ready()
  const results: { name: string; problems: string[] }[] = []
  const wanted = values.only?.split(",")
  const names = Object.keys(TESTS).filter((n) => !wanted || wanted.includes(n))

  for (const name of names) {
    // Fresh teams per test, so grants, files and notes of one test never reach another.
    const a = await createTeam(plan, seeded, `Race A ${name} ${stamp}`)
    const b = await createTeam(plan, seeded, `Race B ${name} ${stamp}`)
    const aLead = (await api.teams()).teams.find((t) => t.id === a.id)?.lead_agent_id as string
    await seedFiles(b, [{ path: "shared.md", content: "B's shared file" }])
    const ctx: Ctx = { a, b, aLead, recorder, dir, roots: [] }
    let problems: string[]
    try {
      problems = await TESTS[name](ctx)
    } catch (error) {
      problems = [`harness: ${message(error)}`]
    }
    // Every task the test started must also tell a consistent story in its events.
    for (const root of ctx.roots) {
      const detail = await waitIdle(root, 15_000)
      const events = await streamed(recorder, detail)
      const approvals = (await api.approvals()).approvals.filter((x) => x.root_run_id === root)
      const agents = (await api.agents()).filter((x) => x.team_id === detail.team_id)
      const t: Transcript = {
        caseId: name, teamKey: "race", rep: 1, rootRunId: root, detail, approvals, outbox: [], agents, events,
        partnerSlug: b.slug, second: null, error: null, seconds: 0,
      }
      const checks = grade(t, { rubric: "" }).filter((c: Check) => c.name === "orchestration" || c.name === "safety")
      for (const c of checks.filter((x) => !x.pass)) problems.push(`${c.name} in ${root}: ${c.detail}`)
      writeTaskLog(dir, `${name}__${root}`, { "events.jsonl": events, "detail.json": detail, "approvals.json": approvals, "checks.json": checks })
    }
    problems.push(...(await replayCheck(ctx)))
    for (const team of [a, b]) await api.deleteTeam(team.id).catch(() => undefined)
    results.push({ name, problems })
    console.log(`${problems.length ? "FAIL" : "ok  "} ${name}${problems.length ? `\n       ${problems.join("\n       ")}` : ""}`)
  }
  recorder.stop()
  const lines = [`# Race and access checks ${stamp}`, "", "| test | result |", "|---|---|"]
  for (const r of results) lines.push(`| ${r.name} | ${r.problems.length ? `FAIL: ${r.problems.join("; ")}` : "ok"} |`)
  lines.push("", `Stream reconnects: ${recorder.reconnects.length}`)
  writeFileSync(join(dir, "report.md"), lines.join("\n"))
  console.log(`Report: ${join(dir, "report.md")}`)
}

await main()
