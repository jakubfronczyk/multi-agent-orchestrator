import { cn } from "cn"

/** A row of pill toggles; exactly one is on. */
export function FilterPills<K extends string>({
  options,
  value,
  onChange,
  label,
  showLabel,
}: {
  options: [K, string][]
  value: K
  onChange: (next: K) => void
  label: string
  /** Prints the label before the pills, where two rows of pills would read as one. */
  showLabel?: boolean
}) {
  return (
    <div role="group" aria-label={label} className="flex flex-wrap items-center gap-1">
      {showLabel && <span className="mr-1 text-sm text-muted-foreground">{label}</span>}
      {options.map(([key, text]) => (
        <button
          key={key}
          type="button"
          aria-pressed={value === key}
          title={text}
          onClick={() => onChange(key)}
          className={cn(
            "h-8 max-w-[24ch] truncate rounded-full px-3 text-sm",
            value === key
              ? "bg-primary text-primary-foreground"
              : "text-sidebar-foreground ring-1 ring-border ring-inset hover:text-foreground hover:ring-input",
          )}
        >
          {text}
        </button>
      ))}
    </div>
  )
}
