import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { RouterProvider } from "react-router/dom"
import { TooltipProvider } from "@/components/ui/tooltip"
import { onResync, startEventHub } from "@/events/event-hub"
import { startTheme } from "@/lib/theme"
import { router } from "@/app/router"
import "./index.css"

const queryClient = new QueryClient({
  defaultOptions: {
    // Freshness comes from events, not from focus or timers.
    queries: { refetchOnWindowFocus: false, retry: 1 },
  },
})

startTheme()
startEventHub()
onResync(() => void queryClient.invalidateQueries())

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider>
        <RouterProvider router={router} />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
)
