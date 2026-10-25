const clock = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hourCycle: "h23" })
const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short" })

/** "14:09" in the viewer's time zone. */
export function clockTime(iso: string): string {
  return clock.format(new Date(iso))
}

/** "Today at 14:09", "Yesterday at 14:09", or "Mon 19 Oct at 14:09". */
export function dayTime(iso: string): string {
  const date = new Date(iso)
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  const days = Math.floor((today.getTime() - new Date(date).setHours(0, 0, 0, 0)) / 86_400_000)
  const day = days === 0 ? "Today" : days === 1 ? "Yesterday" : weekday.format(date)
  return `${day} at ${clockTime(iso)}`
}
