// Copy Whity's design tokens and fonts from a whity-core checkout into
// src/vendor/whity-tokens, and record which commit they came from.
//
//   node scripts/sync-tokens.mjs ../whity-core
//
// Vendored until whity-agent can install @whity-org/tokens from GitHub
// Packages in CI (see README, "Design tokens"). Never edit the copies by hand.
import { cpSync, mkdirSync, readdirSync, writeFileSync } from "node:fs"
import { execFileSync } from "node:child_process"
import { join, resolve } from "node:path"

const core = resolve(process.argv[2] ?? "../whity-core")
const source = join(core, "packages", "tokens")
const target = resolve("src/vendor/whity-tokens")

mkdirSync(join(target, "generated"), { recursive: true })
mkdirSync(join(target, "fonts"), { recursive: true })
for (const file of ["theme.css", "base.css", "fonts.css"]) {
  cpSync(join(source, "generated", file), join(target, "generated", file))
}
for (const file of readdirSync(join(source, "fonts"))) {
  cpSync(join(source, "fonts", file), join(target, "fonts", file))
}

const commit = execFileSync("git", ["-C", core, "log", "-1", "--format=%H", "--", "packages/tokens"]).toString().trim()
writeFileSync(join(target, "VERSION"), `whity-core packages/tokens at ${commit}\n`)
console.log(`Synced tokens from ${core} (${commit.slice(0, 8)})`)
