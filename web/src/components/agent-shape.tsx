import { cn } from "cn"

// Agents are told apart by shape, so two agents never depend on colour alone.
const SHAPES = {
  diamond: <path d="M8 1.5 14.5 8 8 14.5 1.5 8z" />,
  circle: <circle cx="8" cy="8" r="6.2" />,
  square: <rect x="2.2" y="2.2" width="11.6" height="11.6" rx="2.2" />,
  triangle: <path d="M8 1.8 14.6 13.8H1.4z" />,
  hexagon: <path d="M8 1.5 13.6 4.75v6.5L8 14.5 2.4 11.25v-6.5z" />,
}

const SHAPE_BY_ICON: Record<string, keyof typeof SHAPES> = {
  compass: "diamond",
  "book-open": "circle",
  pen: "square",
  send: "triangle",
}

// Full class names, so Tailwind finds them; keys are the colour names agents are saved with.
const COLOUR_CLASS: Record<string, string> = {
  indigo: "text-indigo-600 dark:text-indigo-400",
  teal: "text-teal-600 dark:text-teal-400",
  amber: "text-amber-600 dark:text-amber-400",
  rose: "text-rose-600 dark:text-rose-400",
  sky: "text-sky-600 dark:text-sky-400",
  emerald: "text-emerald-600 dark:text-emerald-400",
  violet: "text-violet-600 dark:text-violet-400",
  orange: "text-orange-600 dark:text-orange-400",
  slate: "text-slate-600 dark:text-slate-400",
}

const SIZE = { sm: "size-3", md: "size-4", lg: "size-5" }

export function AgentShape({
  icon,
  colour,
  size = "sm",
  pulse,
  className,
}: {
  icon: string
  colour?: string
  size?: keyof typeof SIZE
  pulse?: boolean
  className?: string
}) {
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden="true"
      className={cn(
        "flex-none fill-current",
        SIZE[size],
        (colour && COLOUR_CLASS[colour]) ?? "text-muted-foreground",
        pulse && "animate-pulse",
        className,
      )}
    >
      {SHAPES[SHAPE_BY_ICON[icon] ?? "hexagon"]}
    </svg>
  )
}
