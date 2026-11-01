import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react"
import { Button } from "@whity/ui/button"
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@whity/ui/select"
import { Textarea } from "@whity/ui/textarea"
import { connectionName, listEngines, newChat, openApprovals, sendChat, stopChat, type ChatEvent, type Connection, type Engine } from "./api"
import { useI18n } from "./i18n"

/** A line of the conversation. Whatever the model wrote is shown as plain text, never as markup. */
export type Entry =
  | { role: "you"; text: string }
  | { role: "whity"; text: string }
  | { role: "tool"; name: string }
  | { role: "error"; text: string }

/** Add one event from a running turn: text grows the answer in progress. */
export function applyEvent(entries: Entry[], event: ChatEvent): Entry[] {
  const last = entries[entries.length - 1]
  switch (event.kind) {
    case "text":
      return last?.role === "whity"
        ? [...entries.slice(0, -1), { role: "whity", text: last.text + event.text }]
        : [...entries, { role: "whity", text: event.text }]
    case "tool":
      return [...entries, { role: "tool", name: event.name }]
    case "error":
      return [...entries, { role: "error", text: event.message }]
  }
}

function usable(connection: Connection) {
  return connection.persisted
}

/**
 * Chat with Whity on a tool this computer already has (WHIT-824). The turn
 * runs in Rust; this screen sends words and shows what comes back. Changes wait
 * for approval in the workspace, so this links there rather than being a second
 * inbox. The conversation isn't saved here. No design card exists yet (WHIT-821).
 */
export function ChatPanel({ connections }: { connections: Connection[] }) {
  const { t } = useI18n()
  const [engines, setEngines] = useState<Engine[] | null>(null)
  const [engine, setEngine] = useState<string | null>(null)
  const [connection, setConnection] = useState<string | null>(null)
  const [entries, setEntries] = useState<Entry[]>([])
  const [message, setMessage] = useState("")
  const [running, setRunning] = useState(false)
  const bottom = useRef<HTMLDivElement>(null)

  useEffect(() => {
    listEngines().then(setEngines, () => setEngines([]))
  }, [])

  const workspaces = connections.filter(usable)
  const chosenEngine = engine ?? engines?.[0]?.id ?? null
  const chosenConnection = workspaces.some((c) => c.id === connection) ? connection : (workspaces[0]?.id ?? null)

  useEffect(() => bottom.current?.scrollIntoView({ block: "nearest" }), [entries])

  const fail = (e: unknown) => setEntries((current) => [...current, { role: "error", text: String(e) }])

  const send = async (event?: FormEvent) => {
    event?.preventDefault()
    const text = message.trim()
    if (text === "" || running || chosenEngine === null || chosenConnection === null) return
    setMessage("")
    setRunning(true)
    setEntries((current) => [...current, { role: "you", text }])
    try {
      await sendChat(chosenEngine, chosenConnection, text, (update) => setEntries((current) => applyEvent(current, update)))
    } catch (e) {
      fail(e)
    } finally {
      setRunning(false)
    }
  }

  const onKey = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) void send(event)
  }

  const startOver = async () => {
    await newChat().catch(fail)
    setEntries([])
  }

  // Changing what answers starts a new conversation, in both places.
  const change = (set: (value: string) => void) => (value: string) => {
    set(value)
    void startOver()
  }

  if (engines === null) return null
  if (engines.length === 0) return <p className="text-muted-foreground">{t("chat.noEngine")}</p>
  if (workspaces.length === 0) {
    return <p className="text-muted-foreground">{t(connections.length === 0 ? "tools.signInFirst" : "chat.needsKeychain")}</p>
  }

  return (
    <section className="flex min-h-0 flex-1 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        {workspaces.length > 1 && (
          <Select value={chosenConnection ?? undefined} onValueChange={change(setConnection)} disabled={running}>
            <SelectTrigger aria-label={t("chat.workspace")} className="min-w-0 flex-1">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {workspaces.map((c) => (
                <SelectItem key={c.id} value={c.id}>
                  {c.agent === null ? connectionName(c) : t("chat.asAgent", { agent: connectionName(c) })}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <Select value={chosenEngine ?? undefined} onValueChange={change(setEngine)} disabled={running}>
          <SelectTrigger aria-label={t("chat.engine")}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {engines.map((e) => (
              <SelectItem key={e.id} value={e.id}>
                {t("chat.through", { tool: e.name })}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="ms-auto flex gap-2">
          {chosenConnection !== null && (
            <Button variant="outline" onClick={() => openApprovals(chosenConnection).catch(fail)}>
              {t("chat.approvals")}
            </Button>
          )}
          <Button variant="outline" disabled={running || entries.length === 0} onClick={startOver}>
            {t("chat.new")}
          </Button>
        </span>
      </div>

      <div aria-live="polite" className="flex min-h-48 flex-1 flex-col gap-3 overflow-y-auto rounded-card border border-border bg-card p-4">
        {entries.length === 0 && <p className="text-muted-foreground">{t("chat.intro")}</p>}
        {entries.map((entry, index) => (
          <EntryLine key={index} entry={entry} />
        ))}
        {running && <p className="text-xs text-muted-foreground">{t("chat.working")}</p>}
        <div ref={bottom} />
      </div>

      <form onSubmit={send} className="flex items-end gap-2">
        <Textarea
          dir="auto"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={onKey}
          rows={2}
          aria-label={t("chat.message")}
          placeholder={t("chat.placeholder")}
          className="min-w-0 flex-1"
        />
        {running ? (
          <Button variant="outline" onClick={() => stopChat().catch(fail)}>
            {t("chat.stop")}
          </Button>
        ) : (
          <Button type="submit" disabled={message.trim() === ""}>
            {t("chat.send")}
          </Button>
        )}
      </form>
    </section>
  )
}

function EntryLine({ entry }: { entry: Entry }) {
  const { t } = useI18n()
  switch (entry.role) {
    case "you":
      return (
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">{t("chat.you")}</span>
          <p dir="auto" className="whitespace-pre-wrap">
            {entry.text}
          </p>
        </div>
      )
    case "whity":
      return (
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">{t("app.title")}</span>
          <p dir="auto" className="whitespace-pre-wrap">
            {entry.text}
          </p>
        </div>
      )
    case "tool":
      return (
        <p className="text-xs text-muted-foreground">
          {t("chat.usedTool")} <span dir="ltr" className="font-mono">{entry.name}</span>
        </p>
      )
    case "error":
      return (
        <p role="alert" dir="auto" className="whitespace-pre-wrap text-error-text">
          {entry.text}
        </p>
      )
  }
}
