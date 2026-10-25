import type { ReactNode } from "react"
import { cn } from "cn"

/** The design's centred page column: 720px, or 880px when `wide`; 16px gutters on phones. */
export function Page({
  title,
  lede,
  wide,
  children,
}: {
  title: string
  lede?: ReactNode
  wide?: boolean
  children?: ReactNode
}) {
  return (
    <div
      className={cn(
        "mx-auto flex w-full flex-col gap-6 px-4 pt-6 pb-16 md:px-6 md:pt-10",
        wide ? "max-w-[880px]" : "max-w-[720px]",
      )}
    >
      <header>
        <h1 className="m-0 text-[26px] leading-tight font-medium tracking-tight text-balance md:text-3xl">
          {title}
        </h1>
        {lede && <p className="mt-2 mb-0 max-w-[62ch] text-foreground/75">{lede}</p>}
      </header>
      {children}
    </div>
  )
}
