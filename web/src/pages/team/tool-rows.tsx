import { useQuery } from "@tanstack/react-query"
import { cn } from "cn"
import { toolsQuery } from "@/api/meta"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import type { AgentInput } from "@/generated/AgentInput"
import type { Decision } from "@/generated/Decision"
import type { ToolInfo } from "@/generated/ToolInfo"
import type { ToolName } from "@/generated/ToolName"
import { DECISION_LABEL, TOOL_LABEL } from "@/lib/copy"
import { TOOL_DETAIL } from "@/lib/team"
import { Field } from "./fields"
import type { Problem } from "./form"

const DECISIONS: (Decision | "off")[] = ["off", "allow", "ask", "deny"]

/** The tool rows a role can have, each with its Off / Allow / Ask me / Deny control. */
export function ToolRows({
  role,
  draft,
  setDraft,
  problem,
}: {
  role: AgentInput["role"]
  draft: AgentInput
  setDraft: (next: AgentInput) => void
  problem: Problem | null
}) {
  const tools = useQuery(toolsQuery)
  const rows = (tools.data?.tools ?? []).filter((tool) =>
    role === "lead" ? tool.lead !== "forbidden" : tool.lead !== "only",
  )
  return (
    <Field label="Tools" error={problem?.field === "tools" ? problem.message : null}>
      <div className="flex flex-col">
        {rows.map((tool) => (
          <ToolRow
            key={tool.name}
            tool={tool}
            role={role}
            value={draft.tools[tool.name] ?? "off"}
            allowlist={tools.data?.allowlist ?? []}
            onChange={(next) => {
              const nextTools = { ...draft.tools }
              if (next === "off") delete nextTools[tool.name]
              else nextTools[tool.name] = next
              setDraft({ ...draft, tools: nextTools })
            }}
          />
        ))}
      </div>
    </Field>
  )
}

function ToolRow({
  tool,
  role,
  value,
  allowlist,
  onChange,
}: {
  tool: ToolInfo
  role: AgentInput["role"]
  value: Decision | "off"
  allowlist: string[]
  onChange: (next: Decision | "off") => void
}) {
  const name = tool.name as ToolName
  // The Lead's own tools (Hand off, Plan) are always on; external tools never run unasked.
  const choices = DECISIONS.filter((d) => !(d === "off" && role === "lead" && tool.lead === "only"))
  const label = (d: Decision | "off") => (d === "off" ? "Off" : DECISION_LABEL[d])
  return (
    <div className="flex flex-wrap items-center gap-3 border-b border-border py-3 last:border-b-0">
      <span className="min-w-[200px] flex-1 text-sm">
        {TOOL_LABEL[name] ?? tool.name}
        <small className="block text-sm text-muted-foreground">
          {TOOL_DETAIL[name] ?? tool.description}
          {role === "lead" && tool.lead === "trusted_only" && ". Trusted notes only: untrusted notes are invisible to the Lead."}
        </small>
      </span>
      <div role="group" aria-label={TOOL_LABEL[name]} className="grid max-w-full auto-cols-fr grid-flow-col gap-0.5 rounded-md bg-muted p-0.5">
        {choices.map((d) => {
          const blocked = d === "allow" && tool.effect === "external"
          const button = (
            <button
              key={d}
              type="button"
              aria-pressed={value === d}
              aria-disabled={blocked || undefined}
              onClick={() => !blocked && onChange(d)}
              className={cn(
                "h-7 rounded-sm px-2.5 text-sm whitespace-nowrap text-sidebar-foreground hover:text-foreground",
                value === d && "bg-card font-medium ring-1 ring-border",
                value === d && d === "deny" && "text-violet-700 dark:text-violet-400",
                value === d && d === "ask" && "text-amber-800 dark:text-amber-300",
                value === d && d === "allow" && "text-foreground",
                value === d && d === "off" && "text-muted-foreground",
                blocked && "cursor-not-allowed opacity-35 hover:text-sidebar-foreground",
              )}
            >
              {label(d)}
            </button>
          )
          return blocked ? (
            <Tooltip key={d}>
              <TooltipTrigger asChild>{button}</TooltipTrigger>
              <TooltipContent>Sending can only be Ask me or Deny</TooltipContent>
            </Tooltip>
          ) : (
            button
          )
        })}
      </div>
      {name === "http_fetch" && (
        <div className="flex basis-full flex-col gap-2 rounded-md bg-muted px-4 py-3 text-sm text-muted-foreground">
          <span>Allowed sites, set in the app's config. Each site also covers its subdomains.</span>
          <div className="flex flex-wrap gap-2">
            {allowlist.length > 0 ? (
              allowlist.map((site) => (
                <span key={site} className="rounded-full bg-card px-2 py-0.5 text-xs text-sidebar-foreground ring-1 ring-border ring-inset">
                  {site}
                </span>
              ))
            ) : (
              <span>None</span>
            )}
          </div>
          <span>A site that isn't on this list always asks you, even when this is set to Allow.</span>
        </div>
      )}
    </div>
  )
}
