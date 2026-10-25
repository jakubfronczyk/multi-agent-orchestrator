import type { QueryClient, QueryKey } from "@tanstack/react-query"

/**
 * Refetches the matching queries with a request that starts now. React Query joins a first
 * fetch still in flight instead of restarting it, and that fetch began before whatever made
 * the data stale, so a fetch with no data yet is cancelled first.
 */
export async function refetchNow(client: QueryClient, queryKey?: QueryKey): Promise<void> {
  await client.cancelQueries({ queryKey, predicate: (query) => query.state.data === undefined })
  await client.invalidateQueries({ queryKey })
}
