import { createBrowserRouter } from "react-router"
import { AppShell } from "./app-shell"
import { ActivityPage } from "@/pages/activity"
import { DrillPage } from "@/pages/drill"
import { FilesPage } from "@/pages/files"
import { HomePage } from "@/pages/home"
import { NeedsPage } from "@/pages/needs"
import { NotesPage } from "@/pages/notes"
import { SentPage } from "@/pages/sent"
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
      { path: "/files", element: <FilesPage /> },
      { path: "/sent", element: <SentPage /> },
      { path: "/activity", element: <ActivityPage /> },
      { path: "/drill", element: <DrillPage /> },
      { path: "*", element: <Placeholder title="Page not found" /> },
    ],
  },
])
