import { invoke } from "@tauri-apps/api/core"

/**
 * The webview's whole view of the native side. Tokens never cross this line:
 * signing in, refreshing and keychain storage all happen in Rust
 * (src-tauri/src), and what comes back is only what the screen shows.
 */
export interface Connection {
  id: string
  /** The workspace's MCP endpoint, for example https://app.whity.dev/mcp. */
  resource: string
  /** The workspace's own name for itself (its resource_name), when it gives one. */
  name: string | null
  /** The authorization server that signed the person in, for example https://id.whity.dev. */
  issuer: string
  /** Whether the sign-in is kept in the OS keychain, or lasts until the app closes. */
  persisted: boolean
}

/** Error codes the native side answers with; anything else is a free-text reason. */
export type SignInError = "invalid_address" | "no_sign_in" | "cancelled" | "timed_out" | "keychain"

export const listConnections = () => invoke<Connection[]>("list_connections")
export const signIn = (address: string) => invoke<Connection>("sign_in", { address })
export const signOut = (id: string) => invoke<void>("sign_out", { id })
