import type { AgentInput } from "../src/generated/AgentInput.ts"
import type { AgentRecord } from "../src/generated/AgentRecord.ts"
import type { Decision } from "../src/generated/Decision.ts"
import type { ToolName } from "../src/generated/ToolName.ts"
import { api } from "./api.ts"
import { SEEDED_TEAM } from "./fixtures.ts"
import { allows } from "./util.ts"

type Tools = AgentInput["tools"]

/** A test team: seeded agents copied into a new team, with changes. */
export interface TeamPlan {
  key: string
  about: string
  /** Set on a domain team: its Lead gets the server's prompt for a new team, after this line. */
  job?: string
  build: (seeded: Seeded) => AgentInput[]
}

export interface Seeded {
  teamId: string
  lead: AgentRecord
  bySlug: (slug: string) => AgentRecord
}

export interface ReadyTeam {
  key: string
  id: string
  /** Other teams' agents reach this team's folder as `teams/<slug>/`. */
  slug: string
  writerId: string | null
  leadId: string
  /** The writer can read the inbox too, so a scripted inbox read then save raises a card. */
  writerReadsInbox: boolean
}

const input = (agent: AgentRecord, changes: Partial<AgentInput> = {}): AgentInput => ({
  role: agent.role,
  slug: agent.slug,
  name: agent.name,
  description: agent.description,
  system_prompt: agent.system_prompt,
  model: agent.model,
  ...(agent.reasoning_effort ? { reasoning_effort: agent.reasoning_effort } : {}),
  icon: agent.icon,
  colour: agent.colour,
  tools: agent.tools,
  delegate_targets: agent.delegate_targets,
  ...changes,
})

const withTool = (tools: Tools, tool: ToolName, decision: Decision): Tools => ({ ...tools, [tool]: decision })

const SPECIALIST_TOOLS: ToolName[] = [
  "memory_read",
  "memory_search",
  "memory_write",
  "file_list",
  "file_read",
  "file_write",
  "inbox_list",
  "inbox_read",
  "http_fetch",
  "csv_query",
]

const desk = ({ lead, bySlug }: Seeded, messengerSend: Decision = "ask") => [
  input(bySlug("reader")),
  input(bySlug("writer")),
  input(bySlug("messenger"), { tools: withTool(bySlug("messenger").tools, "send_message", messengerSend) }),
  input(lead),
]

export const TEAMS: TeamPlan[] = [
  { key: "access-desk", about: "A copy of the seeded team: Lead, Reader, Writer, Messenger", build: (s) => desk(s) },
  {
    key: "do-it-all",
    about: "Lead plus one agent with every tool on Allow; sending can only be Ask",
    build: ({ lead, bySlug }) => {
      const tools = Object.fromEntries(SPECIALIST_TOOLS.map((tool) => [tool, "allow"])) as Tools
      return [
        input(bySlug("reader"), {
          slug: "operator",
          name: "Operator",
          description: "Reads, writes and sends; does whatever the task needs.",
          system_prompt:
            "You handle access-desk work end to end: read the inbox, files and notes, save files, fetch web pages and send messages when the task needs it. Report exactly what you did.",
          tools: withTool(tools, "send_message", "ask"),
        }),
        input(lead, { delegate_targets: ["operator"] }),
      ]
    },
  },
  { key: "locked-down", about: "The seeded team with the Messenger's send set to Deny", build: (s) => desk(s, "deny") },
  { key: "lonely-lead", about: "A Lead with no teammates", build: ({ lead }) => [input(lead, { delegate_targets: [] })] },
  {
    key: "finance-desk",
    about: "Checks and approves supplier invoices; never pays",
    job: "Your team is the finance desk: it checks supplier invoices against the payment policy in its folder.",
    build: (s) => desk(s),
  },
  {
    key: "hr-desk",
    about: "Onboards new hires: start dates, managers, welcome emails",
    job: "Your team is the HR desk: it onboards new hires using the hire list and checklist in its folder.",
    build: (s) => desk(s),
  },
  {
    key: "research-desk",
    about: "Reads web pages and writes short briefs",
    job: "Your team is the research desk: it reads web pages and writes short, sourced briefs.",
    build: (s) => desk(s),
  },
]

/** The seeded team's agents; every copy starts from them so prompts and models match. */
export async function loadSeeded(): Promise<Seeded> {
  const { teams } = await api.teams()
  const team = teams.find((t) => t.name === SEEDED_TEAM && t.deleted_at === null)
  if (!team) throw new Error(`No live team named "${SEEDED_TEAM}"`)
  const agents = (await api.agents()).filter((a) => a.team_id === team.id)
  const bySlug = (slug: string) => {
    const agent = agents.find((a) => a.slug === slug)
    if (!agent) throw new Error(`"${SEEDED_TEAM}" has no agent "${slug}"`)
    return agent
  }
  return { teamId: team.id, lead: bySlug("lead"), bySlug }
}

/**
 * A new team per task, so notes and files of one task never reach another. Specialists are created
 * before the Lead is replaced, since the server refuses delegate targets that do not exist yet.
 */
export async function createTeam(plan: TeamPlan, seeded: Seeded, name: string): Promise<ReadyTeam> {
  const team = await api.createTeam(name)
  // The server gives a new team's Lead its default prompt; a domain team keeps it, so the eval
  // never runs on a stale copy of that text.
  const defaultPrompt = (await api.agents()).find((a) => a.id === team.lead_agent_id)?.system_prompt ?? ""
  let writerId: string | null = null
  let writerReadsInbox = false
  const agents = plan.build(seeded).sort((a, b) => Number(a.role === "lead") - Number(b.role === "lead"))
  for (const agent of agents) {
    if (agent.role === "lead") {
      const prompt = plan.job ? { system_prompt: `${plan.job} ${defaultPrompt}` } : {}
      await api.replaceAgent(team.lead_agent_id, { ...agent, ...prompt, team_id: team.id })
    } else {
      const created = await api.createAgent({ ...agent, team_id: team.id })
      if (allows(created.tools, "file_write")) {
        writerId = created.id
        writerReadsInbox = allows(created.tools, "inbox_read")
      }
    }
  }
  return { key: plan.key, id: team.id, slug: team.slug, writerId, leadId: team.lead_agent_id, writerReadsInbox }
}
