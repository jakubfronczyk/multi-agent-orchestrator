import { Link } from "react-router"
import { Page } from "@/components/page"

export function NotFound() {
  return (
    <Page title="Page not found">
      <div className="flex flex-col items-center gap-1 rounded-2xl px-6 py-10 text-center ring-1 ring-border ring-inset">
        <p className="m-0 text-sm text-muted-foreground">Nothing lives at this address.</p>
        <Link to="/" className="text-sm text-sidebar-foreground underline underline-offset-[3px] hover:text-foreground">
          Home
        </Link>
      </div>
    </Page>
  )
}
