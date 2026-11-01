## whity-agent

- Design: whity-core `docs/design/AGENT-PERSONAS-AND-WHITY-AGENT.md`, part B. The decision register (`docs/design/00-decision-register.md` in whity-core) is the source of truth for visual and interaction decisions.
- Tokens stay in Rust. The webview never receives an access or refresh token, and no command takes a file path from the webview.
- Secrets go to the OS keychain only (`src-tauri/src/store.rs`). No plaintext fallback, ever.
- Never write a bearer token into another tool's config file. A tool either signs in itself (URL only) or uses the stdio bridge.
- Every config edit to another tool: show the diff, ask, back up, write atomically, and keep a way back.
- Every visible string has English and Arabic (`src/i18n`); use logical CSS properties; numbers and addresses stay left to right.
- Design tokens only (`@whity/tokens`), components from `@whity/ui`, never raw colours or Tailwind palette classes. Noto Sans; no font weight 700. One filled main button per view.
- Every PR adds a `changelog.d` fragment, and its title starts with its WHIT ID.
