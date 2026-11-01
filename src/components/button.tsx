import type { ButtonHTMLAttributes } from "react"

/**
 * The two buttons this app uses, with the class recipe of @whity-org/ui's
 * Button (docs/design/components/Button). `main` is the one action a view is
 * for and the only thing on the plane: a hard offset down-left that never
 * mirrors in RTL, hence the physical `ml`. Every other button is `outline`.
 *
 * Kept local until whity-agent can install @whity-org/ui (see README).
 */
const base =
  "inline-flex h-9 shrink-0 items-center justify-center gap-1.5 rounded-control border px-3.5 text-sm leading-none font-medium whitespace-nowrap outline-none select-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50"

const variants = {
  main: "mb-[3px] ml-[3px] border-transparent bg-primary text-primary-foreground shadow-plane-primary transition-[box-shadow,translate] duration-150 ease-out hover:translate-x-px hover:-translate-y-px hover:shadow-plane-primary-hover active:-translate-x-[3px] active:translate-y-[3px] active:shadow-none",
  outline: "border-input bg-card text-foreground hover:bg-muted",
} as const

export function Button({
  variant = "outline",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: keyof typeof variants }) {
  return <button type="button" className={`${base} ${variants[variant]} ${className}`} {...props} />
}
