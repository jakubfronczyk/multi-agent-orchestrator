import type { QueryKey } from "@tanstack/react-query"
import type { ToolName } from "@/generated/ToolName"
import { toolOf } from "./event-hub"
import { useInvalidateOn } from "./use-invalidate-on"

/**
 * Refetches `queryKey` whenever a `toolName` call finishes. `tool_call_finished` names no tool
 * for a successful call, so the hub's record of requests finds it; a call requested before the
 * app started is unknown and refetches too, since it may be one.
 */
export function useInvalidateOnToolFinished(queryKey: QueryKey, toolName: ToolName) {
  useInvalidateOn(queryKey, (event) => {
    if (event.type !== "tool_call_finished") return false
    const tool = toolOf(event.data.call_id)
    return tool === undefined || tool === toolName
  })
}
