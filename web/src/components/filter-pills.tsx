import { cn } from "cn"

/** A row of pill toggles; exactly one is on. */
export function FilterPills<K extends string>({
  options,
  value,
  onChange,
  label,
}: {
  options: [K, string][]
  value: K
  onChange: (next: K) => void
  label: string
}) {
  return (
    <div role="group" aria-label={label} className="flex flex-wrap gap-1">
      {options.map(([key, text]) => (
        <button
          key={key}
          type="button"
          aria-pressed={value === key}
          onClick={() => onChange(key)}
          className={cn(
            "h-8 rounded-full px-3 text-sm",
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
