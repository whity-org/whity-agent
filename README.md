# whity-agent

Whity's desktop companion. It finds the AI tools on your computer (Claude Code, Codex CLI, Cursor, VS Code, Gemini CLI and others), connects them to your Whity workspace over MCP, and lets you chat with Whity.

Design: [Agent personas, presets and whity-agent](https://github.com/whity-org/whity-core/blob/develop/docs/design/AGENT-PERSONAS-AND-WHITY-AGENT.md) in whity-core, part B. Tasks: WHIT-822 to WHIT-824 (Taskaat, section "Agents").

## What works today

- **Sign in to a workspace.** Type its address (`app.whity.dev`). The app reads the workspace's Protected Resource Metadata, finds its authorization server (Whity ID), uses whity-agent's published client document at Whity ID (`https://id.whity.dev/clients/whity-agent.json`), or registers once as a public client at another authorization server, and signs you in in your browser with OAuth 2.1 + PKCE and a one-time loopback redirect on `127.0.0.1`.
- **Tokens stay out of reach.** Every network call and token lives in Rust (`src-tauri/src`). The webview only sees the workspace's name and address. Refresh tokens go to the OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service) and nowhere else; with no keychain, the sign-in lasts until the app closes.
- **Find and connect your AI tools.** Claude Code, Codex CLI, Gemini CLI, VS Code, Cursor, Windsurf and Claude Desktop are found by looking at PATH and their config files only, never by running them. Connecting shows exactly what will happen first (the tool's own command, or the file before and after) and does nothing until you confirm. Tools get the workspace's URL and sign in themselves; no token is ever written. File edits touch only our entry, refuse a file that changed since you reviewed it or isn't plain JSON, keep a backup, write atomically, and can be undone (`src-tauri/src/tools.rs`, `config_writer.rs`, `linking.rs`). The tool list mirrors `local` in whity-core's `@whity-org/features/mcp`.
- English and Arabic (right to left), light and dark, with Whity's design tokens.

- **A local bridge for tools that can't sign in themselves.** Windsurf and Claude Desktop run `whity-agent mcp --connection <id>`, a stdio MCP server that relays to your workspace. Their config holds that command and nothing secret; the sign-in stays in the keychain (`src-tauri/src/bridge.rs`).

Next: chat and signed updates (WHIT-824).

## Develop

You need Node 22, and Rust from `rust-toolchain.toml` (rustup installs it). On Linux, the Tauri system libraries (see `.github/workflows/ci.yml`).

```bash
npm install
npm run tauri dev
```

Checks, as CI runs them:

```bash
npm run typecheck && npm test
cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
npm run changelog:check
```

## Design tokens

`src/vendor/whity-tokens` is a copy of whity-core's `@whity-org/tokens` (CSS and fonts; `VERSION` says which commit). The package is published to GitHub Packages, which this public repository's CI can't read yet. Update the copy with `npm run tokens:sync -- ../whity-core`; never edit it by hand. Use tokens only, never raw colours.

Screens follow whity-core's `docs/design`. whity-agent's own screens have no design cards yet; they are on the brand team's list (WHIT-821).

## Changes

Add a fragment to `changelog.d/` with every PR (see its README).

## Licence

MIT.
