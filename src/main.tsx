import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { App } from "./App"
import { I18nProvider } from "./i18n"
import "./styles.css"

// Light or dark follows the OS, as the tokens' `.dark` class expects.
const dark = window.matchMedia("(prefers-color-scheme: dark)")
const applyTheme = () => document.documentElement.classList.toggle("dark", dark.matches)
applyTheme()
dark.addEventListener("change", applyTheme)

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <I18nProvider>
      <App />
    </I18nProvider>
  </StrictMode>,
)
