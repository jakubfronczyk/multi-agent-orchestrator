import { useEffect, useState } from "react"
import { useQuery } from "@tanstack/react-query"
import { Outlet, useNavigate } from "react-router"
import { Menu, Plus } from "lucide-react"
import { agentsQuery } from "@/api/agents"
import { toolsQuery } from "@/api/meta"
import { Button } from "@/components/ui/button"
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet"
import { Sidebar } from "./sidebar"

/** Sidebar at 821px and up; below that a top bar and a drawer holding the same sidebar. */
export function AppShell() {
  const [drawer, setDrawer] = useState(false)
  const navigate = useNavigate()

  // Agents and tools back slug, shape and label lookups on every screen.
  useQuery(agentsQuery)
  useQuery(toolsQuery)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault()
        setDrawer(false)
        void navigate("/")
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [navigate])

  return (
    <div className="grid h-full grid-cols-1 md:grid-cols-[256px_minmax(0,1fr)]">
      <aside className="hidden min-w-0 md:block">
        <Sidebar />
      </aside>

      <Sheet open={drawer} onOpenChange={setDrawer}>
        <SheetContent
          side="left"
          showCloseButton={false}
          className="w-[min(288px,85vw)] gap-0 border-0 bg-sidebar p-0 shadow-lg md:hidden"
        >
          <SheetTitle className="sr-only">Navigation</SheetTitle>
          <SheetDescription className="sr-only">Pages, running tasks and history</SheetDescription>
          <Sidebar onNavigate={() => setDrawer(false)} />
        </SheetContent>
      </Sheet>

      <main className="flex h-full min-w-0 flex-col overflow-hidden">
        <div className="flex items-center gap-2 px-3 py-2 md:hidden">
          <Button variant="ghost" size="icon" aria-label="Open menu" onClick={() => setDrawer(true)}>
            <Menu className="size-[18px]" />
          </Button>
          <b className="mr-auto text-lg font-semibold tracking-tight">Orchestrator</b>
          <Button variant="ghost" size="icon" aria-label="New task" onClick={() => void navigate("/")}>
            <Plus className="size-[18px]" />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          <Outlet />
        </div>
      </main>
    </div>
  )
}
