import { useEffect, useState, type FormEvent } from "react"
import { listConnections, signIn, signOut, type Connection, type SignInError } from "./api"
import { Button } from "./components/button"
import { useI18n, type MessageKey } from "./i18n"

const ERROR_KEYS: Record<SignInError, MessageKey> = {
  invalid_address: "errors.invalidAddress",
  no_sign_in: "errors.noSignIn",
  cancelled: "errors.cancelled",
  timed_out: "errors.timedOut",
  keychain: "errors.keychain",
}

/**
 * The first screen: the workspaces this computer is signed in to, and
 * signing in to another. No design card exists for whity-agent yet; it is on
 * the brand team's design-asks list (WHIT-821), so this is built from the
 * tokens and the Button recipe only, to be replaced by the designed screens.
 */
export function App() {
  const { t, toggle } = useI18n()
  const [connections, setConnections] = useState<Connection[]>([])
  const [address, setAddress] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    listConnections().then(setConnections, (e: unknown) => setError(String(e)))
  }, [])

  const describe = (e: unknown): string => {
    const code = String(e)
    return code in ERROR_KEYS ? t(ERROR_KEYS[code as SignInError]) : t("errors.signInFailed", { reason: code })
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const connection = await signIn(address)
      setConnections((current) => [...current.filter((c) => c.id !== connection.id), connection])
      setAddress("")
    } catch (e) {
      setError(describe(e))
    } finally {
      setBusy(false)
    }
  }

  const remove = async (id: string) => {
    await signOut(id)
    setConnections((current) => current.filter((c) => c.id !== id))
  }

  return (
    <main className="mx-auto flex min-h-full max-w-xl flex-col gap-6 bg-background px-6 py-8 text-foreground">
      <header className="flex items-center justify-between">
        <h1 className="text-lg font-semibold">{t("app.title")}</h1>
        <Button onClick={toggle}>{t("app.language")}</Button>
      </header>

      <section className="flex flex-col gap-3">
        <h2 className="text-base font-semibold">{t("connections.title")}</h2>
        <p className="text-muted-foreground">{t("connections.intro")}</p>

        {connections.length === 0 ? (
          <p className="text-muted-foreground">{t("connections.empty")}</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {connections.map((c) => (
              <li key={c.id} className="flex items-center justify-between gap-3 rounded-card border border-border bg-card p-4">
                <div className="flex min-w-0 flex-col gap-1">
                  <span className="truncate font-medium">{c.name ?? new URL(c.resource).host}</span>
                  <span className="truncate text-xs text-muted-foreground" dir="ltr">
                    {new URL(c.resource).host}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {t("connections.via", { issuer: new URL(c.issuer).host })} ·{" "}
                    {t(c.persisted ? "connections.kept" : "connections.sessionOnly")}
                  </span>
                </div>
                <Button onClick={() => remove(c.id)}>{t("connections.signOut")}</Button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <form onSubmit={submit} className="flex flex-col gap-2">
        <label htmlFor="address" className="font-medium">
          {t("connections.address")}
        </label>
        <input
          id="address"
          dir="ltr"
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          placeholder="app.whity.dev"
          autoComplete="url"
          className="h-9 rounded-control border border-input bg-card px-3 text-start outline-none focus-visible:outline-2 focus-visible:outline-ring"
        />
        {error !== null && (
          <p role="alert" className="text-error-text">
            {error}
          </p>
        )}
        <div className="pt-1">
          <Button variant="main" type="submit" disabled={busy || address.trim() === ""}>
            {busy ? t("connections.signingIn") : t("connections.signIn")}
          </Button>
        </div>
      </form>
    </main>
  )
}
