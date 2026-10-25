import { UserRound } from "lucide-react"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { setTheme, useTheme, type ThemeChoice } from "@/lib/theme"

const CHOICES: [ThemeChoice, string][] = [
  ["light", "Light"],
  ["dark", "Dark"],
  ["system", "System"],
]

/** The sidebar's user card; it opens the Light / Dark / System choice. */
export function Appearance() {
  const theme = useTheme()
  return (
    <Popover>
      <PopoverTrigger className="flex w-full items-center gap-3 rounded-md p-2 text-left hover:bg-sidebar-accent">
        <span className="grid size-8 flex-none place-items-center rounded-full bg-secondary ring-1 ring-border ring-inset">
          <UserRound className="size-4" />
        </span>
        <span className="text-sm font-medium text-foreground">You</span>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" className="flex w-(--radix-popover-trigger-width) flex-col gap-2 p-3">
        <small className="text-sm text-muted-foreground">Appearance</small>
        <ToggleGroup
          type="single"
          spacing={0}
          value={theme}
          onValueChange={(value) => value && setTheme(value as ThemeChoice)}
          className="grid w-full grid-cols-3 gap-0.5 rounded-md bg-secondary p-0.5"
        >
          {CHOICES.map(([value, label]) => (
            <ToggleGroupItem
              key={value}
              value={value}
              className="h-7 rounded-sm text-sm text-sidebar-foreground data-[state=on]:bg-card data-[state=on]:font-medium data-[state=on]:text-foreground data-[state=on]:shadow-xs data-[state=on]:ring-1 data-[state=on]:ring-border"
            >
              {label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </PopoverContent>
    </Popover>
  )
}
