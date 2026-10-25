import { useEffect } from "react"

/** Names the browser tab after the page, so screen readers and history show where you are. */
export function useDocumentTitle(title: string) {
  useEffect(() => {
    document.title = title ? `${title}, Orchestrator` : "Orchestrator"
  }, [title])
}
