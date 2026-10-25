import { createBrowserRouter } from "react-router"
import { AppShell } from "./app-shell"
import { NeedsPage } from "@/pages/needs"
import { Placeholder } from "@/pages/placeholder"

export const router = createBrowserRouter([
  {
    element: <AppShell />,
    children: [
      { path: "/", element: <Placeholder title="New task" issue="02" /> },
      { path: "/needs", element: <NeedsPage /> },
      { path: "/tasks", element: <Placeholder title="Tasks" issue="06" /> },
      { path: "/tasks/:rootId", element: <Placeholder title="Task" issue="03" /> },
      { path: "/team", element: <Placeholder title="Your team" issue="07" /> },
      { path: "/notes", element: <Placeholder title="Team notes" issue="08" /> },
      { path: "/files", element: <Placeholder title="Files" issue="09" /> },
      { path: "/sent", element: <Placeholder title="Sent messages" issue="10" /> },
      { path: "/activity", element: <Placeholder title="Activity" issue="11" /> },
      { path: "/drill", element: <Placeholder title="Security drill" issue="12" /> },
      { path: "*", element: <Placeholder title="Page not found" /> },
    ],
  },
])
