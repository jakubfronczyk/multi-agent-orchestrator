import { createBrowserRouter } from "react-router"
import { AppShell } from "./app-shell"
import { HomePage } from "@/pages/home"
import { NeedsPage } from "@/pages/needs"
import { NotesPage } from "@/pages/notes"
import { TaskThreadPage } from "@/pages/task-thread"
import { TasksPage } from "@/pages/tasks"
import { TeamPage } from "@/pages/team/team-page"
import { Placeholder } from "@/pages/placeholder"

export const router = createBrowserRouter([
  {
    element: <AppShell />,
    children: [
      { path: "/", element: <HomePage /> },
      { path: "/needs", element: <NeedsPage /> },
      { path: "/tasks", element: <TasksPage /> },
      { path: "/tasks/:rootId", element: <TaskThreadPage /> },
      { path: "/team", element: <TeamPage /> },
      { path: "/notes", element: <NotesPage /> },
      { path: "/files", element: <Placeholder title="Files" issue="09" /> },
      { path: "/sent", element: <Placeholder title="Sent messages" issue="10" /> },
      { path: "/activity", element: <Placeholder title="Activity" issue="11" /> },
      { path: "/drill", element: <Placeholder title="Security drill" issue="12" /> },
      { path: "*", element: <Placeholder title="Page not found" /> },
    ],
  },
])
