import { useEffect, useRef, type ReactNode } from "react"
import { useLocation } from "react-router"
import { useDocumentTitle } from "@/lib/use-document-title"

/** The design's centred page column: 880px, with 16px gutters on phones. */
export function Page({
  title,
  lede,
  children,
}: {
  title: string
  lede?: ReactNode
  children?: ReactNode
}) {
  useDocumentTitle(title)
  const heading = useRef<HTMLHeadingElement>(null)
  const location = useLocation()
  // After an in-app navigation, focus starts at the new page's heading unless something in the
  // page took it already; the first load keeps the browser's default.
  useEffect(() => {
    if (location.key === "default") return
    if (!document.querySelector("main")?.contains(document.activeElement)) heading.current?.focus()
  }, [location.key])
  return (
    <div className="mx-auto flex w-full max-w-[880px] flex-col gap-6 px-4 pt-6 pb-16 md:px-6 md:pt-10">
      <header>
        <h1 ref={heading} tabIndex={-1} className="m-0 outline-none text-[26px] leading-tight font-medium tracking-tight text-balance md:text-3xl">
          {title}
        </h1>
        {lede && <p className="mt-2 mb-0 max-w-[62ch] text-foreground/75">{lede}</p>}
      </header>
      {children}
    </div>
  )
}
