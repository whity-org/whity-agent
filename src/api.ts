import { Channel, invoke } from "@tauri-apps/api/core"

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

/** How a tool is connected: its own add command, a URL-only config entry, or (later) the bridge. */
export type Method = "command" | "file" | "bridge"

export interface Tool {
  id: string
  name: string
  method: Method
  /** Where its executable was found on PATH. */
  binary: string | null
  config: string | null
  installed: boolean
  /** Ids of the workspaces whity-agent connected it to. */
  connections: string[]
}

/** What connecting will do, shown to the person before anything happens. */
export type Proposal =
  | { kind: "command"; argv: string[]; server: string }
  | { kind: "file"; server: string; plan: { path: string; before: string; after: string; beforeHash: string } }
  | { kind: "nothing"; server: string }

export const listTools = () => invoke<Tool[]>("list_tools")
export const planConnect = (tool: string, connection: string) => invoke<Proposal>("plan_connect", { tool, connection })
export const connectTool = (tool: string, connection: string, approvedHash: string | null) =>
  invoke<void>("connect_tool", { tool, connection, approvedHash })
export const disconnectTool = (tool: string, connection: string) => invoke<void>("disconnect_tool", { tool, connection })
export const restoreTool = (tool: string, connection: string) => invoke<void>("restore_tool", { tool, connection })

/** A tool on this computer that can answer a chat. */
export interface Engine {
  id: string
  name: string
}

/** What arrives during a chat turn. Engine session ids stay on the native side. */
export type ChatEvent = { kind: "text"; text: string } | { kind: "tool"; name: string } | { kind: "error"; message: string }

export const listEngines = () => invoke<Engine[]>("list_engines")
export const sendChat = (engine: string, connection: string, message: string, onUpdate: (event: ChatEvent) => void) => {
  const onEvent = new Channel<ChatEvent>()
  onEvent.onmessage = onUpdate
  return invoke<void>("send_chat", { engine, connection, message, onEvent })
}
export const stopChat = () => invoke<void>("stop_chat")
export const newChat = () => invoke<void>("new_chat")
export const openApprovals = (connection: string) => invoke<void>("open_approvals", { connection })
