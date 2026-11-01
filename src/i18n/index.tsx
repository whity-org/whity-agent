import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react"
import en from "./en.json"
import ar from "./ar.json"

export type Locale = "en" | "ar"
export type MessageKey = keyof typeof en

export const catalogs: Record<Locale, Record<MessageKey, string>> = { en, ar }

/** `{name}` placeholders filled from `values`; a missing key falls back to English. */
export function translate(locale: Locale, key: MessageKey, values: Record<string, string> = {}): string {
  const template = catalogs[locale][key] ?? catalogs.en[key] ?? key
  return template.replace(/\{(\w+)\}/g, (match, name: string) => values[name] ?? match)
}

/** The OS language decides the first run; the person's choice is kept after that. */
export function initialLocale(stored: string | null, languages: readonly string[]): Locale {
  if (stored === "en" || stored === "ar") return stored
  return languages.some((lang) => lang.toLowerCase().startsWith("ar")) ? "ar" : "en"
}

const STORAGE_KEY = "whity-agent.locale"

interface I18n {
  locale: Locale
  t: (key: MessageKey, values?: Record<string, string>) => string
  toggle: () => void
}

const I18nContext = createContext<I18n | null>(null)

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(() => {
    let stored: string | null = null
    try {
      stored = localStorage.getItem(STORAGE_KEY)
    } catch {
      // Storage can be unavailable; the OS language decides then.
    }
    return initialLocale(stored, navigator.languages ?? [navigator.language])
  })

  useEffect(() => {
    document.documentElement.lang = locale
    document.documentElement.dir = locale === "ar" ? "rtl" : "ltr"
    try {
      localStorage.setItem(STORAGE_KEY, locale)
    } catch {
      // Not remembered; harmless.
    }
  }, [locale])

  const t = useCallback((key: MessageKey, values?: Record<string, string>) => translate(locale, key, values), [locale])
  const toggle = useCallback(() => setLocale((current) => (current === "ar" ? "en" : "ar")), [])
  const value = useMemo(() => ({ locale, t, toggle }), [locale, t, toggle])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n(): I18n {
  const context = useContext(I18nContext)
  if (context === null) throw new Error("useI18n outside I18nProvider")
  return context
}
