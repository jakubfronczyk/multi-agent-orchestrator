# web

React app for the orchestrator: start tasks, watch them run live, answer approval cards,
edit teams. Talks only to the backend's `/api`.

React 19, Vite 8, TypeScript 6, Tailwind 4, shadcn/Radix, TanStack Query, React Router,
Oxlint.

## Data flow

```
                 GET /api/...                       GET /api/events (SSE)
backend ──────────────────────▶ TanStack Query     ─────────────────────▶ event-hub
                                  snapshot                                  │ one EventSource,
                                  + last_event_id                           │ buffer, replay
                                       │                                    │
                                       └──────────▶ useLiveState ◀──────────┘
                                                    fold events newer than the snapshot
                                                          │
                                                        pages
```

- **Snapshots, then events.** Each REST snapshot carries `last_event_id`; `useLiveState`
  folds every later event onto it, each once. Queries never go stale by age or window focus
  (`staleTime: Infinity`); only events invalidate them.
- **One stream.** `src/events/event-hub.ts` holds the single `EventSource`, buffers recent
  events, and reconnects with `Last-Event-ID` so the server replays the gap. A gap the
  buffer cannot cover, or a database reset, triggers a refetch of the snapshots.
- **Types come from Rust.** `src/generated/` is written by `ts-rs` from the backend's types
  and never edited by hand. CI fails if it differs from the Rust code.

## Layout

```
src/
  main.tsx                 query client, event hub, theme, router
  app/                     shell, sidebar, routes
  pages/                   home, tasks, task-thread, needs (approvals), team, notes,
                           files, sent, activity, drill
  pages/team/              agent editor, tool rows (allow / ask / deny), file access grants
  components/              composer, call rows, approval slips, trust pills, dialogs
  components/ui/           shadcn primitives
  api/                     one typed fetch module per backend resource
  events/                  event hub, live-state fold, invalidate-on-event hooks
  lib/                     thread state and view, custody ("how this happened"), team scope
  generated/               API types from Rust; do not edit
eval/                      live and scripted agent eval, see eval/README.md
```

## Commands

```bash
pnpm install
pnpm dev          # Vite on :5173, proxies /api to 127.0.0.1:8787 (start the server first)
pnpm build        # tsc -b && vite build
pnpm lint         # oxlint, and typecheck the eval harness
```

`just up` from the repo root starts the server and this app together. Typecheck with
`tsc -b`; `tsc -p .` checks nothing, because the root tsconfig only references the others.

The `/api` proxy in `vite.config.ts` flushes SSE headers immediately. Without that, Node
holds them until the first body write, and the `EventSource` would not open until the 15 s
keep-alive.
