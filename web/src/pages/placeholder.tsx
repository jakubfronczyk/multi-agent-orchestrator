import { Page } from "@/components/page"

/** Stands in for a screen until its issue is built. */
export function Placeholder({ title, issue }: { title: string; issue?: string }) {
  return (
    <Page title={title}>
      <div className="flex flex-col items-center gap-1 rounded-2xl px-6 py-10 text-center ring-1 ring-border ring-inset">
        <b className="font-medium">Not built yet</b>
        <p className="m-0 text-sm text-muted-foreground">
          {issue ? `This screen arrives with issue ${issue}.` : "Nothing lives at this address."}
        </p>
      </div>
    </Page>
  )
}
