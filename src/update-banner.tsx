import { useEffect, useState } from "react"
import { Button } from "@whity/ui/button"
import { checkForUpdate, installUpdate } from "./api"
import { useI18n } from "./i18n"

const DAY = 24 * 60 * 60 * 1000

/**
 * Updates download quietly on start and once a day, and wait here for the
 * person to choose when to restart. A failed check says nothing: the app works
 * as it is, and the next check tries again.
 */
export function UpdateBanner() {
  const { t } = useI18n()
  const [version, setVersion] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const check = () => {
      checkForUpdate().then(
        (ready) => setVersion((current) => ready ?? current),
        () => undefined,
      )
    }
    check()
    const timer = setInterval(check, DAY)
    return () => clearInterval(timer)
  }, [])

  if (version === null) return null

  const restart = async () => {
    setBusy(true)
    setFailed(false)
    try {
      await installUpdate()
    } catch {
      setFailed(true)
      setBusy(false)
    }
  }

  return (
    <div role="status" className="flex items-center justify-between gap-3 rounded-card border border-border bg-card p-4">
      <span>
        {t("update.ready")} <span dir="ltr">{version}</span>
        {failed && <span className="block text-error-text">{t("update.failed")}</span>}
      </span>
      <Button variant="outline" disabled={busy} onClick={restart}>
        {t("update.restart")}
      </Button>
    </div>
  )
}
