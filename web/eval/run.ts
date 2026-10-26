// Live eval: runs each case on each of its teams against the local server, answers cards like a
// person, then grades the recorded calls in code and with an LLM judge.
//   node eval/run.ts [--cases a,b] [--teams x,y] [--pairs case:team,case:team] [--reps 1] [--concurrency 2] [--budget 2] [--no-judge]

import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { parseArgs } from "node:util"
import { api } from "./api.ts"
import { CASES, expectationFor, type Case } from "./cases.ts"
import { DOMAIN_CASES } from "./domains.ts"
import { CHECKS, grade, type Check, type Transcript } from "./graders.ts"
import { judge, judgeInput, JUDGE_MODEL, score, type Verdict } from "./judge.ts"
import { EventRecorder } from "./events.ts"
import { message, seedFiles, sleep, streamed, writeTaskLog } from "./setup.ts"
import { createTeam, loadSeeded, TEAMS, type ReadyTeam, type Seeded, type TeamPlan } from "./teams.ts"

const ALL_CASES = [...CASES, ...DOMAIN_CASES]

const POLL_MS = 1500
const TASK_TIMEOUT_MS = 8 * 60 * 1000
const DATA_FILES = ["data/access-policy.md", "data/access-grants.csv", "data/systems.csv"]
const INBOX = join(import.meta.dirname, "../../backend/orchestrator-server/fixtures/inbox.json")

const { values } = parseArgs({
  options: {
    cases: { type: "string" },
    teams: { type: "string" },
    pairs: { type: "string" },
    reps: { type: "string", default: "1" },
    concurrency: { type: "string", default: "2" },
    budget: { type: "string", default: "2" },
    "no-judge": { type: "boolean", default: false },
    /** Every step but Grok: scripted agents, judge input built but not sent. Costs nothing. */
    dry: { type: "boolean", default: false },
  },
})

try {
  process.loadEnvFile(join(import.meta.dirname, "../../.env"))
} catch {
  // The key may come from the shell instead.
}


interface Row {
  transcript: Transcript
  checks: Check[]
  verdict: Verdict | null
  judgeScore: number | null
  judgeError: string | null
}

/** Answers each pending card once; a failed answer is retried on the next poll. */
async function answerCards(
  c: Case,
  pending: { id: string }[],
  answered: Set<string>,
  full: Map<string, Parameters<Case["cards"]>[0]>,
  log: unknown[],
) {
  for (const { id } of pending) {
    if (answered.has(id)) continue
    const card = full.get(id)
    if (!card) continue
    const answer = c.cards(card)
    try {
      await api.answer(id, answer)
      answered.add(id)
      log.push({ at: new Date().toISOString(), card: id, tool: card.tool, target: card.action?.target, ...answer })
    } catch (error) {
      log.push({ at: new Date().toISOString(), card: id, tool: card.tool, ...answer, error: message(error) })
      // 409: the card expired or was cancelled between the read and the answer.
      if (message(error).includes(": 409 ")) answered.add(id)
      else console.warn(`  ${c.id}: card ${id}: ${message(error)}`)
    }
  }
}

/**
 * The scripted start for a dry run: the writer lists its folder, reads each upload and the partner
 * file, then either reads the inbox and saves (a card) or saves cleanly. A case with a follow-up
 * starts on the Lead, since only a Lead's task takes follow-ups.
 */
function dryStart(c: Case, team: ReadyTeam, partnerSlug: string | null): { agentId: string; scenario: string } {
  if (c.followUp || !team.writerId) return { agentId: team.leadId, scenario: "test:hello" }
  const steps: [string, unknown][] = [["file_list", { path: "." }]]
  for (const file of c.uploads ?? []) steps.push(["file_read", { path: `uploads/${file.path}` }])
  const partnerFile = c.partner?.seed[0]
  if (partnerSlug && partnerFile) steps.push(["file_read", { path: `teams/${partnerSlug}/${partnerFile.path}` }])
  if (team.writerReadsInbox) steps.push(["inbox_read", { id: "access-request-1042" }])
  steps.push(["file_write", { path: "dry/run.md", content: "dry run" }])
  return { agentId: team.writerId, scenario: `test:seq:${steps.map(([tool, args]) => `${tool}:${JSON.stringify(args)}`).join("|")}` }
}

/** Runs one case on a fresh copy of its team, to the end, and reads back everything recorded. */
async function runCase(
  c: Case,
  plan: TeamPlan,
  seeded: Seeded,
  rep: number,
  stamp: string,
  recorder: EventRecorder,
  answers: unknown[],
): Promise<Transcript> {
  const started = Date.now()
  const transcript: Transcript = {
    caseId: c.id,
    teamKey: plan.key,
    rep,
    rootRunId: null,
    detail: null,
    approvals: [],
    outbox: [],
    agents: [],
    events: [],
    partnerSlug: null,
    second: null,
    error: null,
    seconds: 0,
  }
  let teamId: string | null = null
  let partnerId: string | null = null
  try {
    // Team names are at most 60 characters.
    const team = await createTeam(plan, seeded, `Eval ${c.id.slice(0, 22)} ${plan.key} ${rep} ${stamp.slice(11)}`.slice(0, 60))
    teamId = team.id
    await seedFiles(team, c.seed ?? [])
    for (const file of c.uploads ?? []) await api.upload(team.id, file.path, file.content)
    let prompt = c.prompt
    if (c.partner) {
      const partnerPlan = TEAMS.find((t) => t.key === c.partner?.team)
      if (!partnerPlan) throw new Error(`Unknown partner team ${c.partner.team}`)
      const partner = await createTeam(partnerPlan, seeded, `Eval ${c.id.slice(0, 22)} partner ${rep} ${stamp.slice(11)}`.slice(0, 60))
      partnerId = partner.id
      transcript.partnerSlug = partner.slug
      await seedFiles(partner, c.partner.seed)
      if (c.partner.access !== "none") await api.setFileAccess(team.id, partner.id, c.partner.access)
      prompt = prompt.replaceAll("{partner}", partner.slug)
    }
    const start = values.dry ? dryStart(c, team, transcript.partnerSlug) : null
    const { run_id: root } = await api.startRun({
      team_id: teamId,
      task: prompt,
      title: `eval ${c.id}`,
      ...(start ? { agent_id: start.agentId, scenario: start.scenario } : {}),
    })
    transcript.rootRunId = root
    const answered = new Set<string>()
    let followUpSent = !c.followUp
    for (;;) {
      if (Date.now() - started > TASK_TIMEOUT_MS) {
        await api.cancel(root).catch(() => undefined)
        transcript.error = "timed out, cancelled"
        break
      }
      const detail = await api.run(root)
      await answerCards(c, detail.pending_approvals, answered, new Map(detail.pending_approvals.map((card) => [card.id, card])), answers)
      const running = detail.runs.some((run) => run.status === "running")
      if (!running && detail.pending_approvals.length === 0) {
        if (followUpSent) break
        followUpSent = true
        await api.followUp(root, c.followUp as string, values.dry ? "test:hello" : undefined)
      }
      await sleep(POLL_MS)
    }
    transcript.detail = await api.run(root)
    if (c.secondTask && !transcript.error) {
      const dry = values.dry
        ? team.writerId
          ? { agent_id: team.writerId, scenario: `test:call:memory_search:${JSON.stringify({ query: "dry", scope: "shared" })}` }
          : { agent_id: team.leadId, scenario: "test:hello" }
        : {}
      const { run_id: second } = await api.startRun({ team_id: teamId, task: c.secondTask, title: `eval ${c.id} second`, ...dry })
      for (;;) {
        if (Date.now() - started > TASK_TIMEOUT_MS) {
          await api.cancel(second).catch(() => undefined)
          transcript.error = "second task timed out, cancelled"
          break
        }
        const detail = await api.run(second)
        await answerCards(c, detail.pending_approvals, answered, new Map(detail.pending_approvals.map((card) => [card.id, card])), answers)
        if (!detail.runs.some((run) => run.status === "running") && detail.pending_approvals.length === 0) break
        await sleep(POLL_MS)
      }
      transcript.second = { rootRunId: second, detail: await api.run(second) }
    }
    transcript.approvals = (await api.approvals()).approvals.filter((a) => a.root_run_id === root)
    transcript.outbox = (await api.outbox(teamId)).messages.filter((m) => m.root_run_id === root)
    transcript.agents = (await api.agents()).filter((a) => a.team_id === teamId)
    transcript.events = await streamed(recorder, transcript.detail)
  } catch (error) {
    transcript.error = message(error)
  } finally {
    for (const id of [teamId, partnerId]) {
      if (id) await api.deleteTeam(id).catch((error: unknown) => console.warn(`  couldn't delete team: ${message(error)}`))
    }
  }
  transcript.seconds = Math.round((Date.now() - started) / 1000)
  return transcript
}

/** 95% Wilson interval for k passes out of n. */
function wilson(k: number, n: number): [number, number] {
  if (n === 0) return [0, 1]
  const z = 1.96
  const p = k / n
  const centre = (p + (z * z) / (2 * n)) / (1 + (z * z) / n)
  const half = (z * Math.sqrt((p * (1 - p)) / n + (z * z) / (4 * n * n))) / (1 + (z * z) / n)
  return [Math.max(0, centre - half), Math.min(1, centre + half)]
}

const money = (micros: number) => `$${(micros / 1_000_000).toFixed(3)}`
const pct = (x: number) => `${Math.round(x * 100)}%`

function report(rows: Row[], stamp: string, skipped: number): string {
  const lines = [`# Eval ${stamp}`, "", `Judge model: ${JUDGE_MODEL}.`, ""]
  if (skipped) lines.push(`Stopped at the budget; ${skipped} tasks not run.`, "")

  lines.push("## Pass rates", "", `| case | team | n | ${CHECKS.join(" | ")} | judge |`, `|${"---|".repeat(CHECKS.length + 4)}`)
  const groups = new Map<string, Row[]>()
  for (const row of rows) {
    const key = `${row.transcript.caseId} | ${row.transcript.teamKey}`
    groups.set(key, [...(groups.get(key) ?? []), row])
  }
  for (const [key, group] of groups) {
    const cells = CHECKS.map((name) => {
      const graded = group.map((r) => r.checks.find((c) => c.name === name)).filter((c): c is Check => c !== undefined)
      if (graded.length === 0) return "-"
      const passed = graded.filter((c) => c.pass).length
      const [low, high] = wilson(passed, graded.length)
      return graded.length === 1 ? (passed ? "ok" : "FAIL") : `${passed}/${graded.length} (${pct(low)}-${pct(high)})`
    })
    const scores = group.map((r) => r.judgeScore).filter((s): s is number => s !== null)
    const judged = scores.length ? pct(scores.reduce((a, b) => a + b, 0) / scores.length) : "-"
    lines.push(`| ${key} | ${group.length} | ${cells.join(" | ")} | ${judged} |`)
  }
  // Where code and judge disagree one of them is wrong: hand-label these first.
  const disagree = rows.filter((row) => {
    const codeFails = row.checks.some((c) => !c.pass && c.name !== "finished")
    const judged = row.judgeScore
    return judged !== null && ((!codeFails && judged < 0.6) || (codeFails && judged >= 0.8))
  })
  if (disagree.length) {
    lines.push("", "## Code and judge disagree", "")
    for (const row of disagree) {
      const failed = row.checks.filter((c) => !c.pass).map((c) => c.name)
      lines.push(`- ${row.transcript.caseId} / ${row.transcript.teamKey} / ${row.transcript.rep}: code ${failed.length ? `fails ${failed.join(", ")}` : "passes"}, judge ${pct(row.judgeScore ?? 0)}`)
    }
  }
  const cost = rows.reduce((sum, row) => sum + (row.transcript.detail?.cost_micros ?? 0), 0)
  lines.push("", `Agent cost: ${money(cost)} for ${rows.length} tasks.`, "", "## Runs", "")

  for (const row of rows) {
    const t = row.transcript
    lines.push(`### ${t.caseId} / ${t.teamKey} / ${t.rep}`, "", `Task \`${t.rootRunId ?? "not started"}\`, ${money(t.detail?.cost_micros ?? 0)}, ${t.seconds}s.`, "")
    for (const check of row.checks.filter((c) => !c.pass)) lines.push(`- **${check.name}**: ${check.detail}`)
    if (row.verdict) {
      const flags = (["task_done", "grounded", "in_scope", "facts_correct"] as const).filter((f) => !row.verdict?.[f])
      if (row.verdict.followed_injection) flags.push("followed_injection" as never)
      lines.push(`- judge ${pct(row.judgeScore ?? 0)}${flags.length ? `, flagged ${flags.join(", ")}` : ""}`)
      for (const issue of row.verdict.issues) lines.push(`  - ${issue}`)
    }
    if (row.judgeError) lines.push(`- judge error: ${row.judgeError}`)
    const answer = t.detail?.turns.map((turn) => turn.summary ?? "(none)").join("\n\n") ?? "(none)"
    lines.push("", "Final answer:", "", `> ${answer.replace(/\n/g, "\n> ")}`, "")
  }
  return lines.join("\n")
}

async function main() {
  const health = await api.health()
  if (health.provider !== "xai") throw new Error(`Server answers free text with ${health.provider}, not xai`)
  const wantedCases = values.cases?.split(",")
  const wantedTeams = values.teams?.split(",")
  for (const id of wantedCases ?? []) if (!ALL_CASES.some((c) => c.id === id)) throw new Error(`Unknown case ${id}`)
  for (const key of wantedTeams ?? []) if (!TEAMS.some((t) => t.key === key)) throw new Error(`Unknown team ${key}`)
  const reps = Number(values.reps)
  const budget = Number(values.budget) * 1_000_000
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)

  const seeded = await loadSeeded()
  const files = await Promise.all(
    DATA_FILES.map(async (path) => {
      const view = await api.file(seeded.teamId, path)
      return `--- ${path}\n${view.kind === "file" ? view.content : ""}`
    }),
  )
  const seedData = [...files, `--- inbox\n${readFileSync(INBOX, "utf8")}`].join("\n")

  const pairs = ALL_CASES.filter((c) => !wantedCases || wantedCases.includes(c.id)).flatMap((c) =>
    Object.keys(c.teams)
      .filter((key) => !wantedTeams || wantedTeams.includes(key))
      .map((key) => ({ c, plan: TEAMS.find((t) => t.key === key) as TeamPlan })),
  )
  const wantedPairs = values.pairs?.split(",")
  for (const pair of wantedPairs ?? []) if (!pairs.some(({ c, plan }) => `${c.id}:${plan.key}` === pair)) throw new Error(`Unknown pair ${pair}`)
  const chosen = wantedPairs ? pairs.filter(({ c, plan }) => wantedPairs.includes(`${c.id}:${plan.key}`)) : pairs
  const jobs = chosen.flatMap((pair) => Array.from({ length: reps }, (_, i) => ({ ...pair, rep: i + 1 })))
  console.log(`${jobs.length} tasks, budget ${money(budget)}, judge ${values["no-judge"] ? "off" : JUDGE_MODEL}`)

  const dir = join(import.meta.dirname, "out", values.dry ? `dry-${stamp}` : stamp)
  mkdirSync(dir, { recursive: true })
  const recorder = new EventRecorder()
  await recorder.ready()
  const rows: Row[] = []
  let spent = 0
  let next = 0
  const worker = async () => {
    while (next < jobs.length && spent < budget) {
      const { c, plan, rep } = jobs[next++]
      const expectation = expectationFor(c, plan.key)
      const answers: unknown[] = []
      const transcript = await runCase(c, plan, seeded, rep, stamp, recorder, answers)
      spent += transcript.detail?.cost_micros ?? 0
      // A dry run must never reach a model; any cost means a path went live, so stop at once.
      if (values.dry && spent > 0) throw new Error(`dry run spent ${money(spent)} on ${c.id}: a call reached the model`)
      const checks = grade(transcript, expectation)
      let verdict: Verdict | null = null
      let judgeError: string | null = null
      if (values.dry && transcript.detail) {
        // Proves the judge prompt builds for this run; nothing is sent.
        const chars = judgeInput(transcript, c, expectation, seedData).length
        if (chars < 200) judgeError = `judge input suspiciously short (${chars} chars)`
      } else if (!values["no-judge"] && transcript.detail) {
        verdict = await judge(transcript, c, expectation, seedData).catch((error: unknown) => {
          judgeError = message(error)
          return null
        })
      }
      // A run that never finished has no answer to judge, so it scores 0 whatever the judge said.
      const finished = checks.find((check) => check.name === "finished")?.pass ?? false
      const judgeScore = verdict ? (finished ? score(verdict, expectation) : 0) : null
      const row: Row = { transcript, checks, verdict, judgeScore, judgeError }
      rows.push(row)
      appendFileSync(join(dir, "results.jsonl"), `${JSON.stringify({ ...row, transcript: { ...transcript, events: undefined } })}\n`)
      writeTaskLog(dir, `${c.id}__${plan.key}__${rep}`, {
        "events.jsonl": transcript.events,
        "detail.json": transcript.detail,
        "approvals.json": transcript.approvals,
        "card-answers.json": answers,
        "outbox.json": transcript.outbox,
        "agents.json": transcript.agents,
        "grade.json": { checks, verdict, judgeError, error: transcript.error, seconds: transcript.seconds },
      })
      const failed = checks.filter((check) => !check.pass).map((check) => check.name)
      console.log(
        `  ${c.id} / ${plan.key} / ${rep}: ${failed.length ? `FAIL ${failed.join(",")}` : "ok"}, judge ${row.judgeScore === null ? "-" : pct(row.judgeScore)}, ${money(transcript.detail?.cost_micros ?? 0)}, spent ${money(spent)}`,
      )
    }
  }
  await Promise.all(Array.from({ length: Math.min(Number(values.concurrency), jobs.length) }, worker))
  recorder.stop()
  writeFileSync(join(dir, "stream.json"), JSON.stringify({ reconnects: recorder.reconnects, duplicates: recorder.duplicates }, null, 2))

  writeFileSync(join(dir, "report.md"), report(rows, stamp, jobs.length - rows.length))
  console.log(`Report: ${join(dir, "report.md")}`)
}

await main()
