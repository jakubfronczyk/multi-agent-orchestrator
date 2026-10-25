import { useRef } from "react"
import type { QueryKey } from "@tanstack/react-query"
import type { ToolName } from "@/generated/ToolName"
import { useInvalidateOn } from "./use-invalidate-on"

/**
 * Refetches `queryKey` whenever a `toolName` call finishes. `tool_call_finished` names no tool
 * for a successful call, so the ids of that tool's requests are remembered to recognise it.
 */
export function useInvalidateOnToolFinished(queryKey: QueryKey, toolName: ToolName) {
  const calls = useRef(new Set<string>())
  useInvalidateOn(queryKey, (event) => {
    if (event.type === "tool_call_requested" && event.data.tool === toolName) calls.current.add(event.data.call_id)
    return event.type === "tool_call_finished" && calls.current.has(event.data.call_id)
  })
}
