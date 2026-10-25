import { useEffect, useState } from "react"
import { useQuery } from "@tanstack/react-query"
import { Outlet, useNavigate } from "react-router"
import { Menu, Plus } from "lucide-react"
import { agentsQuery } from "@/api/agents"
import { toolsQuery } from "@/api/meta"
import { Button } from "@/components/ui/button"
import { Sheet, SheetContent, SheetDescription, SheetTitle } from "@/components/ui/sheet"
import { isMac } from "@/lib/platform"
import { Sidebar } from "./sidebar"

const WIDE = "(min-width: 821px)"

/** Sidebar at 821px and up; below that a top bar and a drawer holding the same sidebar. */
export function AppShell() {
  const [drawer, setDrawer] = useState(false)
  const navigate = useNavigate()

  // Agents and tools back slug, shape and label lookups on every screen.
  useQuery(agentsQuery)
  useQuery(toolsQuery)

  // The drawer is hidden by CSS once the window is wide; Radix would keep it open, the page
  // scroll-locked and the main content inert.
  useEffect(() => {
    const wide = window.matchMedia(WIDE)
    const close = () => wide.matches && setDrawer(false)
    wide.addEventListener("change", close)
    return () => wide.removeEventListener("change", close)
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // Ctrl+K on a Mac is the text fields' "kill line", so only Command counts there.
      const modifier = isMac ? event.metaKey : event.ctrlKey
      if (modifier && event.key.toLowerCase() === "k") {
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
