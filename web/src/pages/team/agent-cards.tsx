import { Plus } from "lucide-react"
import { cn } from "cn"
import { AgentShape } from "@/components/agent-shape"
import type { AgentRecord } from "@/generated/AgentRecord"
import { MODEL_LABEL } from "@/lib/team"

/** The team as cards, the selected one outlined, with "Add a teammate" last. */
export function AgentCards({
  agents,
  selectedId,
  adding,
  onSelect,
  onAdd,
}: {
  agents: AgentRecord[]
  selectedId: string | undefined
  adding: boolean
  onSelect: (agent: AgentRecord) => void
  onAdd: () => void
}) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {agents.map((agent) => (
        <button
          key={agent.id}
          type="button"
          aria-pressed={!adding && selectedId === agent.id}
          onClick={() => onSelect(agent)}
          className={cn(
            "flex min-w-0 flex-col gap-2 rounded-lg bg-card p-4 text-left ring-1 ring-border ring-inset hover:ring-input",
            !adding && selectedId === agent.id && "ring-[1.5px] ring-foreground hover:ring-foreground",
          )}
        >
          <span className="flex items-center gap-2 font-medium">
            <AgentShape icon={agent.icon} colour={agent.colour} size="md" />
            {agent.name}
          </span>
          <p className="m-0 text-sm text-sidebar-foreground">{agent.description || "No description yet."}</p>
          <small className="text-xs text-muted-foreground">
            {MODEL_LABEL[agent.model]}, up to {agent.max_steps} steps
          </small>
        </button>
      ))}
      <button
        type="button"
        aria-pressed={adding}
        onClick={onAdd}
        className={cn(
          "flex min-h-[120px] flex-col items-center justify-center gap-1 rounded-lg text-sidebar-foreground ring-[1.5px] ring-border ring-inset hover:text-foreground hover:ring-input",
          adding && "ring-foreground",
        )}
      >
        <Plus className="size-[18px]" />
        Add a teammate
      </button>
    </div>
  )
}
