import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { RouterProvider } from "react-router/dom"
import { TooltipProvider } from "@/components/ui/tooltip"
import { onResync, startEventHub } from "@/events/event-hub"
import { isApiError } from "@/lib/api"
import { startTheme } from "@/lib/theme"
import { router } from "@/app/router"
import "./index.css"

const queryClient = new QueryClient({
  defaultOptions: {
    // Freshness comes from events (invalidation and resync), not from focus or age. A 4xx
    // answer will not change on retry.
    queries: {
      staleTime: Infinity,
      refetchOnWindowFocus: false,
      retry: (failures, error) => !(isApiError(error) && error.status < 500) && failures < 1,
    },
  },
})

startTheme()
onResync(async () => {
  // React Query joins a first fetch still in flight instead of restarting it, and that fetch may
  // predate the stream; cancelling it makes the invalidation below start a fresh one.
  await queryClient.cancelQueries({ predicate: (query) => query.state.data === undefined })
  await queryClient.invalidateQueries()
})
startEventHub()

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <RouterProvider router={router} />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
)
