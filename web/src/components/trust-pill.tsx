import { cn } from "cn"
import type { Trust } from "@/generated/Trust"

export function TrustPill({ trust, className }: { trust: Trust; className?: string }) {
  return (
    <span
      className={cn(
        "rounded-full px-2 text-xs whitespace-nowrap ring-1 ring-current ring-inset",
        trust === "trusted" ? "text-emerald-700 dark:text-emerald-400" : "text-amber-800 dark:text-amber-300",
        className,
      )}
    >
      {trust === "trusted" ? "Trusted" : "Untrusted"}
    </span>
  )
}
