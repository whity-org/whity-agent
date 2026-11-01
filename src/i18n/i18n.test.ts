import { describe, expect, it } from "vitest"
import en from "./en.json"
import ar from "./ar.json"
import { initialLocale, translate } from "./index"

describe("catalogs", () => {
  it("has an Arabic string for every English one, and no extras", () => {
    expect(Object.keys(ar).sort()).toEqual(Object.keys(en).sort())
  })

  it("keeps the same placeholders in both languages", () => {
    const placeholders = (text: string) => (text.match(/\{\w+\}/g) ?? []).sort()
    for (const key of Object.keys(en) as (keyof typeof en)[]) {
      expect(placeholders(ar[key]), key).toEqual(placeholders(en[key]))
    }
  })

  it("has no empty strings", () => {
    for (const catalog of [en, ar]) {
      for (const [key, value] of Object.entries(catalog)) expect(value.trim(), key).not.toBe("")
    }
  })
})

describe("translate", () => {
  it("fills placeholders", () => {
    expect(translate("en", "connections.via", { issuer: "id.whity.dev" })).toBe("through id.whity.dev")
    expect(translate("ar", "connections.via", { issuer: "id.whity.dev" })).toBe("عبر id.whity.dev")
  })
})

describe("initialLocale", () => {
  it("keeps the stored choice", () => {
    expect(initialLocale("ar", ["en-US"])).toBe("ar")
  })
  it("follows the OS the first time", () => {
    expect(initialLocale(null, ["ar-JO", "en"])).toBe("ar")
    expect(initialLocale(null, ["en-GB"])).toBe("en")
  })
})
