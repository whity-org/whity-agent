//! Chat with Whity on a tool the person already has (WHIT-824, design §5.6,
//! option A): Claude Code or Codex, run headless with the workspace attached
//! through the stdio bridge, so no model key or bill is added and no token
//! reaches the tool.
//!
//! The model gets Whity's tools and nothing else: no files, no shell, none of
//! the person's other MCP servers, hooks or plugins. What it can do in the
//! workspace is what the sign-in can do, and the server still enforces that.
//!
//! The webview names an engine and a connection and sends text. The program,
//! its arguments and the session to resume are all decided here, and the text
//! goes over stdin, never on a command line.

use crate::tools::Environment;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

/// The name the bridge goes by in the engine, so its tools are `mcp__whity__*`.
const SERVER: &str = "whity";

/// Said to the model before the person's words. The workspace's own briefing
/// comes from `init_session`, and the server enforces what it says.
const BRIEFING: &str = "You are chatting with a person through Whity Agent, about their Whity workspace. Use the whity tools for everything about the workspace, and call init_session first. You can't reach this computer's files or run commands. Answer in the person's language, briefly. When a tool says a change is waiting for approval, say so plainly: it hasn't happened yet.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Claude,
    Codex,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Claude, Engine::Codex];

    pub fn id(self) -> &'static str {
        match self {
            Engine::Claude => "claude-code",
            Engine::Codex => "codex",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Engine::Claude => "Claude Code",
            Engine::Codex => "Codex",
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Engine::Claude => "claude",
            Engine::Codex => "codex",
        }
    }

    pub fn from_id(id: &str) -> Option<Engine> {
        Engine::ALL.into_iter().find(|e| e.id() == id)
    }

    /// The engine's program on this computer, found the way a connection finds it.
    pub fn locate(self, env: &Environment) -> Option<PathBuf> {
        env.which(self.binary())
    }

    /// Everything after the program name. `config` is the Claude Code MCP config.
    fn args(self, bridge: &BridgeCommand, config: &Path, session: Option<&str>) -> Vec<String> {
        match self {
            Engine::Claude => claude_args(config, session),
            Engine::Codex => codex_args(bridge, session),
        }
        .unwrap_or_default()
    }

    /// What goes on stdin. Claude Code takes the briefing as a flag; `codex
    /// exec` has none, so it opens the conversation's first message.
    fn prompt(self, message: &str, session: Option<&str>) -> String {
        match (self, session) {
            (Engine::Codex, None) => format!("{BRIEFING}\n\n{message}"),
            _ => message.to_string(),
        }
    }

    fn parse(self, line: &str) -> Vec<ChatEvent> {
        match self {
            Engine::Claude => parse_claude(line),
            Engine::Codex => parse_codex(line),
        }
    }
}

/// How an engine starts the bridge. Holds a connection id, never a token.
pub struct BridgeCommand {
    pub program: String,
    pub connection: String,
}

impl BridgeCommand {
    fn args(&self) -> [&str; 3] {
        ["mcp", "--connection", &self.connection]
    }

    /// The MCP config Claude Code reads, as `--mcp-config` takes it.
    fn claude_config(&self) -> Value {
        json!({ "mcpServers": { SERVER: { "command": self.program, "args": self.args() } } })
    }
}

/// `--tools ""` removes every built-in tool (MCP tools stay), and
/// `--setting-sources ""` the person's hooks, plugins and permission settings.
/// Their sign-in is kept: the subscription is why this option exists.
fn claude_args(config: &Path, session: Option<&str>) -> Option<Vec<String>> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--strict-mcp-config",
        "--tools",
        "",
        "--setting-sources",
        "",
        "--permission-mode",
        "dontAsk",
        "--allowedTools",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(format!("mcp__{SERVER}"));
    args.push("--mcp-config".into());
    args.push(config.display().to_string());
    args.push("--append-system-prompt".into());
    args.push(BRIEFING.into());
    if let Some(session) = session {
        args.push("--resume".into());
        args.push(session.into());
    }
    Some(args)
}

/// `--ignore-user-config` drops the person's other MCP servers and settings and
/// keeps their sign-in. Values are TOML literal strings ('…'), which need no
/// escaping and no double quotes; a path with a `'` can't be written that way.
fn codex_args(bridge: &BridgeCommand, session: Option<&str>) -> Option<Vec<String>> {
    if bridge.program.contains('\'') || bridge.connection.contains('\'') {
        return None;
    }
    let bridge_args = bridge
        .args()
        .iter()
        .map(|a| format!("'{a}'"))
        .collect::<Vec<_>>()
        .join(",");
    let mut args: Vec<String> = vec!["exec".into()];
    if session.is_some() {
        args.push("resume".into());
    }
    args.extend(
        ["--json", "--skip-git-repo-check", "--ignore-user-config"]
            .iter()
            .map(|s| s.to_string()),
    );
    for setting in [
        "sandbox_mode='read-only'".to_string(),
        "web_search='disabled'".to_string(),
        "features.shell_tool=false".to_string(),
        "features.unified_exec=false".to_string(),
        "features.apps=false".to_string(),
        "features.multi_agent=false".to_string(),
        format!("mcp_servers.{SERVER}.command='{}'", bridge.program),
        format!("mcp_servers.{SERVER}.args=[{bridge_args}]"),
        // Exec mode has nobody to ask. Writes are Ask first on the server anyway.
        format!("mcp_servers.{SERVER}.default_tools_approval_mode='approve'"),
    ] {
        args.push("-c".into());
        args.push(setting);
    }
    if let Some(session) = session {
        args.push(session.into());
    }
    args.push("-".into());
    Some(args)
}

/// What the webview hears about a turn (src/api.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatEvent {
    /// More of the answer.
    Text { text: String },
    /// The model used a Whity tool.
    Tool { name: String },
    /// The turn failed, in the engine's words.
    Error { message: String },
    /// The engine's conversation id, kept here to resume. Never sent on.
    #[serde(skip)]
    Session { id: String },
}

/// One line of Claude Code's `stream-json`. Anything unrecognised is skipped:
/// the format grows, and a new event must not break a chat.
pub fn parse_claude(line: &str) -> Vec<ChatEvent> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    match message["type"].as_str() {
        Some("system") if message["subtype"] == "init" => {
            if let Some(id) = message["session_id"].as_str() {
                events.push(ChatEvent::Session { id: id.to_string() });
            }
        }
        Some("stream_event") => {
            let event = &message["event"];
            match event["type"].as_str() {
                Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        events.push(ChatEvent::Text { text: text.into() });
                    }
                }
                Some("content_block_start") if event["content_block"]["type"] == "tool_use" => {
                    if let Some(name) = event["content_block"]["name"].as_str() {
                        let prefix = format!("mcp__{SERVER}__");
                        let name = name.strip_prefix(&prefix).unwrap_or(name);
                        events.push(ChatEvent::Tool { name: name.into() });
                    }
                }
                _ => {}
            }
        }
        Some("result") if message["is_error"] == true => {
            let text = message["result"].as_str().filter(|t| !t.is_empty());
            let fallback = message["subtype"].as_str().unwrap_or("the tool stopped");
            events.push(ChatEvent::Error {
                message: text.unwrap_or(fallback).to_string(),
            });
        }
        _ => {}
    }
    events
}

/// One line of `codex exec --json`. Its `error` events include retries it
/// recovers from, so only `turn.failed` ends a turn with an error.
pub fn parse_codex(line: &str) -> Vec<ChatEvent> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let item = &message["item"];
    match message["type"].as_str() {
        Some("thread.started") => message["thread_id"]
            .as_str()
            .map(|id| ChatEvent::Session { id: id.to_string() })
            .into_iter()
            .collect(),
        Some("item.started") if item["type"] == "mcp_tool_call" => item["tool"]
            .as_str()
            .map(|name| ChatEvent::Tool { name: name.into() })
            .into_iter()
            .collect(),
        Some("item.completed") if item["type"] == "agent_message" => item["text"]
            .as_str()
            .map(|text| ChatEvent::Text { text: text.into() })
            .into_iter()
            .collect(),
        Some("turn.failed") => vec![ChatEvent::Error {
            message: message["error"]["message"]
                .as_str()
                .unwrap_or("the tool stopped")
                .to_string(),
        }],
        _ => Vec::new(),
    }
}

/// Feed an engine's output through its parser, one line at a time. Returns
/// whether it reported an error of its own.
async fn pump(
    output: impl AsyncRead + Unpin,
    parse: impl Fn(&str) -> Vec<ChatEvent>,
    emit: &mut (impl FnMut(ChatEvent) + Send),
) -> bool {
    let mut failed = false;
    let mut lines = BufReader::new(output).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        for event in parse(&line) {
            failed |= matches!(event, ChatEvent::Error { .. });
            emit(event);
        }
    }
    failed
}

/// The last line an engine wrote to stderr, for when it dies without saying why.
async fn last_line(stderr: impl AsyncRead + Unpin) -> String {
    let mut last = String::new();
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if !line.trim().is_empty() {
            last = line;
        }
    }
    last
}

/// One run of a program: `input` on stdin, its stdout through `parse`, until it
/// ends or `stop` fires. Windows gets no console window for it.
pub async fn run_program(
    program: &Path,
    args: &[String],
    cwd: &Path,
    input: String,
    parse: impl Fn(&str) -> Vec<ChatEvent> + Send,
    mut emit: impl FnMut(ChatEvent) + Send,
    stop: oneshot::Receiver<()>,
) -> Result<(), String> {
    std::fs::create_dir_all(cwd).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::tools::no_console(&mut command);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", program.display()))?;

    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let write = async move {
        // An engine that stops early closes the pipe; what it says about that is on stdout.
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    };
    let id = child.id();
    let finish = async {
        let (_, reported, stderr) =
            tokio::join!(write, pump(stdout, parse, &mut emit), last_line(stderr));
        let status = child.wait().await;
        (reported, stderr, status)
    };
    tokio::pin!(finish);

    tokio::select! {
        (reported, stderr, status) = &mut finish => {
            let status = status.map_err(|e| e.to_string())?;
            if status.success() || reported {
                Ok(())
            } else if stderr.is_empty() {
                Err(format!("the tool stopped ({status})"))
            } else {
                Err(stderr)
            }
        }
        _ = stop => {
            kill_tree(id).await;
            Ok(())
        }
    }
}

/// End a program and what it started: a `.cmd` shim runs node, and the engine
/// runs the bridge. The bridge also ends by itself when its stdin closes.
async fn kill_tree(id: Option<u32>) {
    let Some(id) = id else { return };
    let (program, args) = if cfg!(windows) {
        (
            "taskkill",
            vec!["/PID".to_string(), id.to_string(), "/T".into(), "/F".into()],
        )
    } else {
        ("kill", vec!["--".to_string(), format!("-{id}")])
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::tools::no_console(&mut command);
    let _ = command.status().await;
}

/// One turn of a conversation: the arguments, and what goes on stdin.
pub struct Turn {
    pub args: Vec<String>,
    pub input: String,
}

pub fn prepare_turn(
    engine: Engine,
    bridge: &BridgeCommand,
    dir: &Path,
    session: Option<&str>,
    message: &str,
) -> Result<Turn, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // A file rather than inline JSON, which a `.cmd` shim would have to quote.
    let config = dir.join(format!("mcp-{}.json", bridge.connection));
    std::fs::write(&config, bridge.claude_config().to_string()).map_err(|e| e.to_string())?;
    let args = engine.args(bridge, &config, session);
    if args.is_empty() {
        return Err("Whity Agent's folder has a character Codex can't be given".into());
    }
    Ok(Turn {
        args,
        input: engine.prompt(message, session),
    })
}

pub fn parser(engine: Engine) -> impl Fn(&str) -> Vec<ChatEvent> + Send {
    move |line| engine.parse(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> BridgeCommand {
        BridgeCommand {
            program: r"C:\Program Files\Whity Agent\whity-agent.exe".into(),
            connection: "abc123".into(),
        }
    }

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        let at = args.iter().position(|a| a == flag)?;
        args.get(at + 1).map(String::as_str)
    }

    #[test]
    fn claude_code_gets_whity_tools_and_nothing_else() {
        let args = claude_args(Path::new("cfg.json"), None).unwrap();

        assert_eq!(value_after(&args, "--tools"), Some(""));
        assert_eq!(value_after(&args, "--setting-sources"), Some(""));
        assert_eq!(value_after(&args, "--allowedTools"), Some("mcp__whity"));
        assert_eq!(value_after(&args, "--permission-mode"), Some("dontAsk"));
        assert_eq!(value_after(&args, "--mcp-config"), Some("cfg.json"));
        assert!(args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(!args.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn a_later_turn_resumes_the_conversation() {
        let args = claude_args(Path::new("cfg.json"), Some("s-1")).unwrap();
        assert_eq!(value_after(&args, "--resume"), Some("s-1"));

        let args = codex_args(&bridge(), Some("t-1")).unwrap();
        assert_eq!(&args[..2], ["exec", "resume"]);
        assert_eq!(&args[args.len() - 2..], ["t-1", "-"]);
    }

    #[test]
    fn codex_gets_no_shell_and_reads_the_message_from_stdin() {
        let args = codex_args(&bridge(), None).unwrap();

        assert_eq!(&args[..2], ["exec", "--json"]);
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(args.iter().any(|a| a == "--ignore-user-config"));
        assert!(args.iter().any(|a| a == "features.shell_tool=false"));
        assert!(args.iter().any(|a| a == "sandbox_mode='read-only'"));
        assert!(args
            .iter()
            .any(|a| a
                == r"mcp_servers.whity.command='C:\Program Files\Whity Agent\whity-agent.exe'"));
        assert!(args
            .iter()
            .any(|a| a == "mcp_servers.whity.args=['mcp','--connection','abc123']"));
        assert!(
            !args.iter().any(|a| a.contains('"')),
            "no quotes to survive a .cmd shim"
        );
    }

    #[test]
    fn a_path_codex_cant_be_given_is_refused_not_broken() {
        let awkward = BridgeCommand {
            program: r"C:\O'Brien\whity-agent.exe".into(),
            connection: "abc".into(),
        };
        assert!(codex_args(&awkward, None).is_none());

        let dir =
            std::env::temp_dir().join(format!("whity-agent-chat-{}", crate::oauth::random_token()));
        assert!(prepare_turn(Engine::Codex, &awkward, &dir, None, "hi").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_config_holds_a_command_and_a_connection_id_only() {
        let dir =
            std::env::temp_dir().join(format!("whity-agent-chat-{}", crate::oauth::random_token()));
        let turn =
            prepare_turn(Engine::Claude, &bridge(), &dir, None, "hello --dangerous").unwrap();

        assert!(
            !turn.args.iter().any(|a| a.contains("--dangerous")),
            "text never reaches the command line"
        );
        assert_eq!(turn.input, "hello --dangerous");
        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp-abc123.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["mcpServers"]["whity"]["args"],
            json!(["mcp", "--connection", "abc123"])
        );
        assert_eq!(config["mcpServers"]["whity"].as_object().unwrap().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_hears_the_briefing_once() {
        assert!(Engine::Codex.prompt("hi", None).starts_with(BRIEFING));
        assert_eq!(Engine::Codex.prompt("hi", Some("t-1")), "hi");
        assert_eq!(Engine::Claude.prompt("hi", None), "hi");
    }

    #[test]
    fn claude_code_stream_becomes_events() {
        // Shapes as printed by Claude Code 2.1.
        let init = r#"{"type":"system","subtype":"init","session_id":"s-1","tools":[]}"#;
        assert_eq!(
            parse_claude(init),
            vec![ChatEvent::Session { id: "s-1".into() }]
        );

        let delta = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"pong"}}}"#;
        assert_eq!(
            parse_claude(delta),
            vec![ChatEvent::Text {
                text: "pong".into()
            }]
        );

        let tool = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","name":"mcp__whity__search"}}}"#;
        assert_eq!(
            parse_claude(tool),
            vec![ChatEvent::Tool {
                name: "search".into()
            }]
        );

        let failed =
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#;
        assert_eq!(
            parse_claude(failed),
            vec![ChatEvent::Error {
                message: "Not logged in".into()
            }]
        );

        // The whole message repeats what the deltas said; a good result adds nothing.
        for quiet in [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"pong"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"pong"}"#,
            r#"{"type":"system","subtype":"hook_started"}"#,
            r#"{"type":"something_new"}"#,
            "not json",
        ] {
            assert!(parse_claude(quiet).is_empty(), "{quiet}");
        }
    }

    #[test]
    fn codex_stream_becomes_events() {
        let started = r#"{"type":"thread.started","thread_id":"t-1"}"#;
        assert_eq!(
            parse_codex(started),
            vec![ChatEvent::Session { id: "t-1".into() }]
        );

        let tool = r#"{"type":"item.started","item":{"id":"i","type":"mcp_tool_call","server":"whity","tool":"search"}}"#;
        assert_eq!(
            parse_codex(tool),
            vec![ChatEvent::Tool {
                name: "search".into()
            }]
        );

        let text =
            r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"hello"}}"#;
        assert_eq!(
            parse_codex(text),
            vec![ChatEvent::Text {
                text: "hello".into()
            }]
        );

        let failed = r#"{"type":"turn.failed","error":{"message":"401"}}"#;
        assert_eq!(
            parse_codex(failed),
            vec![ChatEvent::Error {
                message: "401".into()
            }]
        );

        // Printed by Codex 0.166 while it retries, and it may yet recover.
        assert!(parse_codex(r#"{"type":"error","message":"Reconnecting... 2/5"}"#).is_empty());
        assert!(parse_codex(r#"{"type":"turn.started"}"#).is_empty());
    }

    #[test]
    fn the_session_id_never_reaches_the_webview() {
        let event = serde_json::to_value(ChatEvent::Text { text: "x".into() }).unwrap();
        assert_eq!(event, json!({ "kind": "text", "text": "x" }));
        assert!(serde_json::to_value(ChatEvent::Session { id: "s".into() }).is_err());
    }

    /// `sort` reads stdin and writes stdout on Windows, macOS and Linux alike.
    fn sort() -> PathBuf {
        PathBuf::from("sort")
    }

    #[tokio::test]
    async fn a_program_gets_the_text_on_stdin_and_its_lines_are_parsed() {
        let (_keep, stop) = oneshot::channel();
        let mut heard = Vec::new();
        let result = run_program(
            &sort(),
            &[],
            &std::env::temp_dir(),
            "bravo\r\nalpha\r\n".to_string(),
            |line| {
                vec![ChatEvent::Text {
                    text: line.trim().to_string(),
                }]
            },
            |event| heard.push(event),
            stop,
        )
        .await;

        result.unwrap();
        assert_eq!(
            heard,
            vec![
                ChatEvent::Text {
                    text: "alpha".into()
                },
                ChatEvent::Text {
                    text: "bravo".into()
                }
            ]
        );
    }

    #[tokio::test]
    async fn a_missing_program_says_so() {
        let (_keep, stop) = oneshot::channel();
        let error = run_program(
            Path::new("whity-agent-no-such-program"),
            &[],
            &std::env::temp_dir(),
            String::new(),
            |_| Vec::new(),
            |_| {},
            stop,
        )
        .await
        .unwrap_err();
        assert!(error.contains("couldn't start"));
    }

    /// Runs the real Claude Code with the real arguments, so it needs it
    /// installed and signed in: `cargo test real_claude_code -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn real_claude_code_streams_an_answer_with_no_tools() {
        let dir =
            std::env::temp_dir().join(format!("whity-agent-chat-{}", crate::oauth::random_token()));
        let program = Engine::Claude
            .locate(&Environment::current())
            .expect("claude on PATH");
        let turn = prepare_turn(
            Engine::Claude,
            &bridge(),
            &dir,
            None,
            "Reply with the single word pong.",
        )
        .unwrap();
        let (_keep, stop) = oneshot::channel();
        let mut heard = Vec::new();

        run_program(
            &program,
            &turn.args,
            &dir,
            turn.input,
            parser(Engine::Claude),
            |e| heard.push(e),
            stop,
        )
        .await
        .unwrap();

        assert!(
            matches!(heard.first(), Some(ChatEvent::Session { .. })),
            "{heard:?}"
        );
        let text: String = heard
            .iter()
            .filter_map(|e| match e {
                ChatEvent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.to_lowercase().contains("pong"), "{heard:?}");
        assert!(
            !heard.iter().any(|e| matches!(e, ChatEvent::Error { .. })),
            "{heard:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
