import type { AgentInput } from "@/generated/AgentInput"
import type { AgentRecord } from "@/generated/AgentRecord"
import type { Decision } from "@/generated/Decision"
import type { Model } from "@/generated/Model"
import type { ReasoningEffort } from "@/generated/ReasoningEffort"
import type { ToolName } from "@/generated/ToolName"

export const MODEL_LABEL: Record<Model, string> = {
  "grok-4.3": "Grok 4.3",
  "grok-4.20-0309-non-reasoning": "Grok 4.20 non-reasoning",
}

export const EFFORT_LABEL: Record<ReasoningEffort, string> = {
  none: "None",
  low: "Low",
  medium: "Medium",
  high: "High",
}

/** What each tool does, in the words of the team editor. */
export const TOOL_DETAIL: Record<ToolName, string> = {
  delegate: "Gives a step to a teammate",
  plan: "Writes and updates the plan",
  memory_read: "Reads a shared note or its own private one",
  memory_search: "Looks through notes",
  memory_write: "Saves a note, shared or private",
  file_list: "Sees which files are in a folder",
  file_read: "Reads a file in your workspace",
  file_write: "Saves a document",
  inbox_list: "Sees what is in your inbox. What it lists counts as from outside.",
  inbox_read: "Reads a message in your inbox. What it reads counts as from outside.",
  http_fetch: "Opens a web page. What it reads counts as from outside.",
  csv_query: "Looks up rows in a table",
  send_message: "Sends email or chat. Can only be Ask me or Deny.",
}

export interface Preset {
  id: "reader" | "writer" | "messenger" | "custom"
  name: string
  detail: string
  icon: string
  colour: string
  tools: Partial<Record<ToolName, Decision>>
}

/** Starting points for a new teammate; nothing about a preset is stored apart from its values. */
export const PRESETS: Preset[] = [
  {
    id: "reader",
    name: "Reader",
    detail: "Reads files and websites",
    icon: "book-open",
    colour: "teal",
    tools: {
      inbox_list: "allow",
      inbox_read: "allow",
      file_list: "allow",
      file_read: "allow",
      http_fetch: "allow",
      csv_query: "allow",
      memory_read: "allow",
      memory_search: "allow",
      memory_write: "allow",
      send_message: "deny",
    },
  },
  {
    id: "writer",
    name: "Writer",
    detail: "Drafts and saves documents",
    icon: "pen",
    colour: "amber",
    tools: { file_read: "allow", file_write: "allow", memory_read: "allow", memory_search: "allow", memory_write: "allow" },
  },
  {
    id: "messenger",
    name: "Messenger",
    detail: "Sends email and chat",
    icon: "send",
    colour: "rose",
    tools: { send_message: "ask", memory_read: "allow", memory_search: "allow" },
  },
  { id: "custom", name: "Custom", detail: "Starts with every tool Off", icon: "hexagon", colour: "slate", tools: {} },
]

/** A slug the server accepts from a display name: lowercase `[a-z0-9_-]`, at most 32. */
export function slugFrom(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 32)
    .replace(/-$/, "")
}

/** The editable part of a saved agent, as `PUT /agents/:id` takes it. */
export function inputOf(agent: AgentRecord): AgentInput {
  return {
    role: agent.role,
    slug: agent.slug,
    name: agent.name,
    description: agent.description,
    system_prompt: agent.system_prompt,
    model: agent.model,
    reasoning_effort: agent.reasoning_effort ?? undefined,
    icon: agent.icon,
    colour: agent.colour,
    tools: agent.tools,
    delegate_targets: agent.delegate_targets,
  }
}

export function presetInput(preset: Preset, name: string): AgentInput {
  return {
    role: "specialist",
    slug: slugFrom(name),
    name,
    description: "",
    system_prompt: "",
    model: "grok-4.3",
    reasoning_effort: "low",
    icon: preset.icon,
    colour: preset.colour,
    tools: { ...preset.tools },
    delegate_targets: [],
  }
}
