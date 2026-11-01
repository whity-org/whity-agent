// Check changelog.d fragments (changelog.d/README.md): `<id>.<section>.md`,
// a known section, and at least one list item. CI runs this on every PR.
import { readdirSync, readFileSync } from "node:fs"

const SECTIONS = ["added", "changed", "deprecated", "removed", "fixed", "security"]
const errors = []
const fragments = readdirSync("changelog.d").filter((name) => name !== "README.md")

for (const name of fragments) {
  const match = /^[A-Za-z0-9-]+\.([a-z]+)\.md$/.exec(name)
  if (match === null || !SECTIONS.includes(match[1])) {
    errors.push(`${name}: name it <id>.<section>.md, with section one of ${SECTIONS.join(", ")}`)
    continue
  }
  const text = readFileSync(`changelog.d/${name}`, "utf8")
  if (!/^- \S/m.test(text)) errors.push(`${name}: needs at least one "- " list item`)
  if (/^#/m.test(text)) errors.push(`${name}: no headings; the section comes from the file name`)
}

if (errors.length > 0) {
  console.error(errors.join("\n"))
  process.exit(1)
}
console.log(`${fragments.length} changelog fragment(s), all valid.`)
