import { useEffect, useRef, useState, type ReactNode } from "react"
import { useQueryClient } from "@tanstack/react-query"
import { ArrowUp, ChevronDown, Eye, Paperclip, Shield, Square, X } from "lucide-react"
import { cn } from "cn"
import { uploadFile } from "@/api/files"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import type { TeamId } from "@/generated/TeamId"
import type { UploadResponse } from "@/generated/UploadResponse"
import { isApiError } from "@/lib/api"

const MODES = {
  ask: { name: "Team settings", icon: Shield, detail: "Follows your team's settings: an action set to Allow runs without asking" },
  draft: {
    name: "Draft only",
    icon: Eye,
    detail: "Reads and drafts. Saving, notes, sending and opening websites are refused without asking you.",
  },
} as const

type Mode = keyof typeof MODES

export interface SubmitOptions {
  draftOnly: boolean
  attachments: UploadResponse[]
}

/** Why an upload was refused, by the status `POST /files` answers with. */
function uploadError(error: unknown): string {
  if (isApiError(error, 409)) return "Too many uploads already share that name. Rename the file and try again."
  if (isApiError(error, 422) && error.message) return `Couldn't read that file: ${error.message}.`
  if (isApiError(error, 413)) return error.message ? `Too big to attach: ${error.message}.` : "That file is too big to attach."
  return error instanceof Error ? `Couldn't attach that file. ${error.message}` : "Couldn't attach that file."
}

/**
 * The message box under a thread and on Home. The text is owned by the caller, so a refused
 * send keeps it; attachments and the mode picker are optional.
 */
export function Composer({
  value,
  onChange,
  onSubmit,
  onStop,
  stopping,
  placeholder,
  label,
  disabled,
  sending,
  action = "send",
  modePicker,
  attach,
  menuBelow,
  autoFocus,
  children,
}: {
  value: string
  onChange: (text: string) => void
  onSubmit: (text: string, options: SubmitOptions) => Promise<boolean>
  onStop?: () => void
  /** A stop was sent and has not answered yet. */
  stopping?: boolean
  placeholder: string
  label: string
  disabled?: boolean
  sending?: boolean
  action?: "send" | "stop"
  modePicker?: boolean
  /** The team an attached file is uploaded to; unset hides attaching. */
  attach?: TeamId
  /** Opens the mode menu downwards, for a composer near the top of the page. */
  menuBelow?: boolean
  autoFocus?: boolean
  /** A line under the box: an error or a hint. */
  children?: ReactNode
}) {
  const [mode, setMode] = useState<Mode>("ask")
  const [attachments, setAttachments] = useState<UploadResponse[]>([])
  // An upload's path names a file in one team's folder, so another team drops the chips.
  const [attachedTo, setAttachedTo] = useState(attach)
  if (attachedTo !== attach) {
    setAttachedTo(attach)
    setAttachments([])
  }
  // Read after an upload resolves: a team switch meanwhile must not get the other team's chip.
  const team = useRef(attach)
  useEffect(() => {
    team.current = attach
  }, [attach])
  const [attachError, setAttachError] = useState<string | null>(null)
  const [uploading, setUploading] = useState(false)
  const fileInput = useRef<HTMLInputElement>(null)
  // Set synchronously, so a second Enter in the same frame finds it before `sending` updates.
  const inFlight = useRef(false)
  // This composer's uploads by team and file identity: a removed chip's file is still in uploads/, so
  // picking it again reuses the upload instead of a POST that would conflict.
  const uploads = useRef(new Map<string, UploadResponse[]>())
  const client = useQueryClient()
  const canSend = !disabled && !sending && !uploading && value.trim().length > 0

  const submit = async () => {
    if (!canSend || inFlight.current) return
    inFlight.current = true
    try {
      // Attachments count only while the composer offers them; a note to a card carries none.
      const ok = await onSubmit(value.trim(), {
        draftOnly: modePicker === true && mode === "draft",
        attachments: attach ? attachments : [],
      })
      if (ok) setAttachments([])
    } finally {
      inFlight.current = false
    }
  }

  const add = (files: UploadResponse[]) =>
    setAttachments((current) => [...current, ...files.filter((file) => !current.some((f) => f.path === file.path))])

  const pick = async (file: File | undefined) => {
    if (!file) return
    setAttachError(null)
    if (!attach) return
    const key = `${attach}:${file.name}:${file.size}:${file.lastModified}`
    const earlier = uploads.current.get(key)
    if (earlier) {
      add(earlier)
      return
    }
    setUploading(true)
    try {
      const uploaded = await uploadFile(attach, file)
      uploads.current.set(key, uploaded.files)
      if (team.current === attach) add(uploaded.files)
      // The upload is now in the team's folder under uploads/.
      void client.invalidateQueries({ queryKey: ["files"] })
    } catch (error) {
      setAttachError(uploadError(error))
    } finally {
      setUploading(false)
    }
  }

  const ModeIcon = MODES[mode].icon
  return (
    <div className="flex flex-col gap-2">
      <div
        data-state={action === "stop" ? "running" : "idle"}
        className={cn(
          "flex flex-col gap-1 rounded-[28px] bg-card py-3 pr-3 pl-4 shadow-xs ring-1 ring-border transition-shadow focus-within:ring-input",
          disabled && action === "send" && "opacity-70",
        )}
      >
        {attach && attachments.length > 0 && (
          <div className="flex flex-wrap gap-2 px-1">
            {attachments.map((file) => (
              <span
                key={file.path}
                className="inline-flex h-7 max-w-full items-center gap-2 rounded-full bg-muted pr-1 pl-2.5 text-sm text-sidebar-foreground"
              >
                <Paperclip className="size-3.5 flex-none" />
                <span className="min-w-0 truncate">{file.path}</span>
                <span className="rounded-full px-1.5 text-xs text-amber-800 ring-1 ring-current ring-inset dark:text-amber-300">
                  {file.readable ? "Untrusted" : "Can't be read"}
                </span>
                <button
                  type="button"
                  aria-label={`Remove ${file.path}`}
                  onClick={() => setAttachments((current) => current.filter((f) => f.path !== file.path))}
                  className="grid size-5.5 flex-none place-items-center rounded-full text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <X className="size-3.5" />
                </button>
              </span>
            ))}
          </div>
        )}
        <textarea
          rows={1}
          aria-label={label}
          placeholder={placeholder}
          value={value}
          // Read-only, not disabled: a disabled field drops focus when a task briefly runs between
          // cards, mid-typing. Sending is still blocked through `canSend`.
          readOnly={disabled}
          aria-disabled={disabled}
          autoFocus={autoFocus}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault()
              void submit()
            }
          }}
          className="field-sizing-content max-h-[180px] min-h-8 resize-none border-0 bg-transparent px-1 pt-1 text-lg/relaxed text-foreground outline-none placeholder:text-muted-foreground/70 aria-disabled:cursor-not-allowed"
        />
        <div className="flex items-center gap-2">
          {attach && (
            <>
              <button
                type="button"
                aria-label="Attach a file"
                title="Attached files count as outside content"
                disabled={uploading || disabled}
                onClick={() => fileInput.current?.click()}
                className="grid size-9 flex-none place-items-center rounded-full text-sidebar-foreground hover:bg-accent hover:text-foreground disabled:opacity-50"
              >
                <Paperclip className="size-[18px]" />
              </button>
              <input
                ref={fileInput}
                type="file"
                hidden
                onChange={(event) => {
                  void pick(event.target.files?.[0])
                  event.target.value = ""
                }}
              />
            </>
          )}
          <span className="flex-1" />
          {modePicker && (
            <DropdownMenu>
              <DropdownMenuTrigger className="inline-flex h-9 items-center gap-2 rounded-full px-3 text-sm text-sidebar-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring">
                <ModeIcon className="size-4" />
                {MODES[mode].name}
                <ChevronDown className="size-4" />
              </DropdownMenuTrigger>
              <DropdownMenuContent
                side={menuBelow ? "bottom" : "top"}
                align="end"
                className="w-[300px] max-w-[calc(100vw-48px)] rounded-lg p-1 shadow-lg"
              >
                <DropdownMenuRadioGroup value={mode} onValueChange={(next) => setMode(next as Mode)}>
                  {(Object.keys(MODES) as Mode[]).map((key) => {
                    const Icon = MODES[key].icon
                    return (
                      <DropdownMenuRadioItem key={key} value={key} className="items-start gap-3 rounded-md p-3 pr-9">
                        <Icon className="mt-0.5 text-sidebar-foreground" />
                        <span className="flex flex-col gap-0.5">
                          <b className="text-sm font-medium">{MODES[key].name}</b>
                          <small className="text-sm/snug text-muted-foreground">{MODES[key].detail}</small>
                        </span>
                      </DropdownMenuRadioItem>
                    )
                  })}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          {action === "stop" ? (
            <button
              type="button"
              aria-label="Stop the task"
              disabled={stopping}
              onClick={onStop}
              className="grid size-9 flex-none place-items-center rounded-full bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-50"
            >
              <Square className="size-3.5 fill-current" />
            </button>
          ) : (
            <button
              type="button"
              aria-label="Send"
              disabled={!canSend}
              onClick={() => void submit()}
              className="grid size-9 flex-none place-items-center rounded-full bg-primary text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-default disabled:opacity-25"
            >
              <ArrowUp className="size-[18px]" />
            </button>
          )}
        </div>
      </div>
      {attachError && (
        <p role="alert" className="m-0 px-4 text-sm text-destructive">
          {attachError}
        </p>
      )}
      {children}
    </div>
  )
}
