/** A record's team, as a small pill. */
export function TeamTag({ name }: { name: string | undefined }) {
  if (!name) return null
  return (
    <span className="inline-block max-w-[16ch] truncate rounded-full bg-muted px-2 py-0.5 align-middle text-xs text-sidebar-foreground">
      {name}
    </span>
  )
}
