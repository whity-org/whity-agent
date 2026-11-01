import { useCallback, useEffect, useRef, useState } from "react"
import {
  connectTool,
  disconnectTool,
  listTools,
  planConnect,
  restoreTool,
  type Connection,
  type Proposal,
  type Tool,
} from "./api"
import { Button } from "./components/button"
import { useI18n } from "./i18n"

interface Review {
  tool: Tool
  connection: Connection
  proposal: Proposal
}

/**
 * The AI tools found on this computer (WHIT-823). Connecting is always two
 * steps: the review shows exactly what will happen (the tool's own command,
 * or the file before and after), and only "Connect" there does it. No design
 * card exists yet (WHIT-821); built from tokens and the Button recipe.
 */
export function ToolsPanel({ connections }: { connections: Connection[] }) {
  const { t } = useI18n()
  const [tools, setTools] = useState<Tool[]>([])
  const [review, setReview] = useState<Review | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = useCallback(() => {
    listTools().then(setTools, (e: unknown) => setError(String(e)))
  }, [])
  useEffect(refresh, [refresh, connections])

  const workspaceName = (id: string) => {
    const c = connections.find((c) => c.id === id)
    return c === undefined ? id : (c.name ?? new URL(c.resource).host)
  }

  const act = async (work: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try {
      await work()
      setReview(null)
      refresh()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  // Planning opens the review; nothing changes until "Connect" there.
  const openReview = async (tool: Tool, connection: Connection) => {
    setBusy(true)
    setError(null)
    try {
      setReview({ tool, connection, proposal: await planConnect(tool.id, connection.id) })
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }
  const installed = tools.filter((tool) => tool.installed)
  const first = connections[0]

  return (
    <section className="flex flex-col gap-3">
      <h2 className="text-base font-semibold">{t("tools.title")}</h2>
      <p className="text-muted-foreground">{t("tools.intro")}</p>
      {connections.length === 0 && <p className="text-muted-foreground">{t("tools.signInFirst")}</p>}
      {installed.length === 0 && <p className="text-muted-foreground">{t("tools.none")}</p>}

      <ul className="flex flex-col gap-2">
        {installed.map((tool) => (
          <li key={tool.id} className="flex flex-col gap-2 rounded-card border border-border bg-card p-4">
            <div className="flex items-center justify-between gap-3">
              <div className="flex min-w-0 flex-col gap-1">
                <span className="font-medium">{tool.name}</span>
                <span className="text-xs text-muted-foreground">
                  {tool.method === "bridge" ? t("tools.needsBridge") : t("tools.signsInItself")}
                </span>
              </div>
              {tool.method !== "bridge" && first !== undefined && !tool.connections.includes(first.id) && (
                <Button disabled={busy} onClick={() => openReview(tool, first)}>
                  {t("tools.connect")}
                </Button>
              )}
            </div>
            {tool.connections.map((id) => (
              <div key={id} className="flex flex-wrap items-center justify-between gap-2 text-sm">
                <span>{t("tools.connectedTo", { workspace: workspaceName(id) })}</span>
                <span className="flex gap-2">
                  {tool.method === "file" && (
                    <Button disabled={busy} onClick={() => act(() => restoreTool(tool.id, id))}>
                      {t("tools.restore")}
                    </Button>
                  )}
                  <Button disabled={busy} onClick={() => act(() => disconnectTool(tool.id, id))}>
                    {t("tools.disconnect")}
                  </Button>
                </span>
              </div>
            ))}
          </li>
        ))}
      </ul>

      {error !== null && (
        <p role="alert" className="text-error-text">
          {error}
        </p>
      )}

      {review !== null && (
        <ReviewSheet
          review={review}
          busy={busy}
          onCancel={() => setReview(null)}
          onConfirm={() =>
            act(() =>
              connectTool(
                review.tool.id,
                review.connection.id,
                review.proposal.kind === "file" ? review.proposal.plan.beforeHash : null,
              ),
            )
          }
        />
      )}
    </section>
  )
}

function ReviewSheet({
  review,
  busy,
  onCancel,
  onConfirm,
}: {
  review: Review
  busy: boolean
  onCancel: () => void
  onConfirm: () => void
}) {
  const { t } = useI18n()
  const { tool, proposal } = review
  const sheet = useRef<HTMLDivElement>(null)
  useEffect(() => sheet.current?.scrollIntoView({ behavior: "smooth", block: "nearest" }), [review])

  return (
    <div ref={sheet} role="dialog" aria-label={t("review.title", { tool: tool.name })} className="flex flex-col gap-3 rounded-card border border-border bg-card p-4">
      <h3 className="font-semibold">{t("review.title", { tool: tool.name })}</h3>

      {proposal.kind === "command" && (
        <>
          <p>{t("review.command", { tool: tool.name })}</p>
          <pre dir="ltr" className="overflow-x-auto rounded-control bg-muted p-3 font-mono text-xs">
            {proposal.argv.join(" ")}
          </pre>
          <p className="text-muted-foreground">{t("review.signIn", { tool: tool.name })}</p>
        </>
      )}

      {proposal.kind === "file" && (
        <>
          {/* The path is isolated left to right (U+2066 to U+2069) so it reads whole inside an Arabic sentence. */}
          <p>{t("review.file", { path: String.fromCharCode(0x2066) + proposal.plan.path + String.fromCharCode(0x2069) })}</p>
          <div className="grid gap-2 sm:grid-cols-2">
            {(
              [
                ["review.before", proposal.plan.before],
                ["review.after", proposal.plan.after],
              ] as const
            ).map(([label, text]) => (
              <div key={label} className="flex min-w-0 flex-col gap-1">
                <span className="text-xs text-muted-foreground">{t(label)}</span>
                <pre dir="ltr" className="max-h-64 overflow-auto rounded-control bg-muted p-3 font-mono text-xs">
                  {text === "" ? "—" : text}
                </pre>
              </div>
            ))}
          </div>
          <p className="text-muted-foreground">{t("review.signIn", { tool: tool.name })}</p>
        </>
      )}

      {proposal.kind === "nothing" && <p>{t("review.nothing", { tool: tool.name })}</p>}
      {proposal.kind === "needsBridge" && <p>{t("tools.needsBridge")}</p>}

      <div className="flex gap-2 pt-1">
        {(proposal.kind === "command" || proposal.kind === "file" || proposal.kind === "nothing") && (
          <Button variant="main" disabled={busy} onClick={onConfirm}>
            {t("review.confirm")}
          </Button>
        )}
        <Button disabled={busy} onClick={onCancel}>
          {t("review.cancel")}
        </Button>
      </div>
    </div>
  )
}
