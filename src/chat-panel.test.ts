import { describe, expect, it } from "vitest"
import { applyEvent, type Entry } from "./chat-panel"

describe("applyEvent", () => {
  it("grows the answer in progress", () => {
    let entries: Entry[] = [{ role: "you", text: "hi" }]
    entries = applyEvent(entries, { kind: "text", text: "Hel" })
    entries = applyEvent(entries, { kind: "text", text: "lo" })
    expect(entries).toEqual([
      { role: "you", text: "hi" },
      { role: "whity", text: "Hello" },
    ])
  })

  it("starts a new answer after a tool", () => {
    let entries: Entry[] = [{ role: "whity", text: "Looking" }]
    entries = applyEvent(entries, { kind: "tool", name: "search" })
    entries = applyEvent(entries, { kind: "text", text: "Found it" })
    expect(entries).toEqual([
      { role: "whity", text: "Looking" },
      { role: "tool", name: "search" },
      { role: "whity", text: "Found it" },
    ])
  })

  it("keeps an error as its own line", () => {
    expect(applyEvent([], { kind: "error", message: "Not logged in" })).toEqual([{ role: "error", text: "Not logged in" }])
  })
})
