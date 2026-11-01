//! `whity-agent mcp --connection <id>`: a local stdio MCP server that relays to
//! a signed-in workspace (WHIT-824), for AI tools that can't sign in themselves
//! (Windsurf, Claude Desktop).
//!
//! The tool's config holds only this command and a connection id. The token
//! stays in the OS keychain and in this process's memory; it is never written
//! to stdout, a log or a file.
//!
//! stdout carries protocol messages only, one JSON-RPC message per line, as MCP's
//! stdio transport requires. Messages are relayed one at a time, in order.

use crate::connections::Connections;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// The per-request protocol version in `params._meta`, from MCP 2026-07-28.
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// The first version that declares itself per request rather than by header.
const FIRST_PER_REQUEST_VERSION: &str = "2026-07-28";
const NOT_SIGNED_IN: i64 = -32001;
const INTERNAL_ERROR: i64 = -32603;
const PARSE_ERROR: i64 = -32700;
const SIGN_IN_AGAIN: &str = "Not signed in to this workspace. Open Whity Agent and sign in again.";

pub struct Bridge<'a> {
    connections: &'a Connections,
    connection_id: String,
    endpoint: String,
    http: reqwest::Client,
    /// A legacy version the workspace negotiated in `initialize`, sent as a header.
    negotiated: Option<String>,
}

impl<'a> Bridge<'a> {
    pub fn new(connections: &'a Connections, connection_id: &str, endpoint: &str) -> Self {
        Self {
            connections,
            connection_id: connection_id.to_string(),
            endpoint: endpoint.to_string(),
            http: reqwest::Client::builder()
                .user_agent(concat!("whity-agent-bridge/", env!("CARGO_PKG_VERSION")))
                // A tool call can take a while; a stuck one must not hang the tool forever.
                .timeout(Duration::from_secs(300))
                .build()
                .expect("an HTTP client"),
            negotiated: None,
        }
    }

    /// Relay stdin to the workspace and its answers to stdout, until stdin closes.
    pub async fn run(&mut self) -> Result<(), String> {
        self.relay(tokio::io::stdin(), tokio::io::stdout()).await
    }

    async fn relay(
        &mut self,
        input: impl AsyncRead + Unpin,
        mut stdout: impl AsyncWrite + Unpin,
    ) -> Result<(), String> {
        let mut lines = BufReader::new(input).lines();
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            if line.trim().is_empty() {
                continue;
            }
            for out in self.handle_line(&line).await {
                stdout
                    .write_all(out.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                stdout.write_all(b"\n").await.map_err(|e| e.to_string())?;
            }
            stdout.flush().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// One message from the tool, and the lines to answer with. A notification
    /// that fails gets no answer: it has no id to answer to.
    pub async fn handle_line(&mut self, line: &str) -> Vec<String> {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![error_line(
                &Value::Null,
                PARSE_ERROR,
                "That isn't valid JSON.",
            )];
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let answer = |code: i64, text: &str| {
            if message.get("id").is_some() {
                vec![error_line(&id, code, text)]
            } else {
                Vec::new()
            }
        };

        let mut response = match self.post(line, &message).await {
            Ok(response) => response,
            Err(reason) => return failed(&reason, answer),
        };
        // An expired or revoked access token: try once more with a fresh one.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.connections.forget_access_token(&self.connection_id);
            response = match self.post(line, &message).await {
                Ok(response) => response,
                Err(reason) => return failed(&reason, answer),
            };
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return answer(NOT_SIGNED_IN, SIGN_IN_AGAIN);
            }
        }

        let status = response.status();
        let event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/event-stream"));
        let body = match response.text().await {
            Ok(body) => body,
            Err(e) => {
                return answer(
                    INTERNAL_ERROR,
                    &format!("The workspace's answer couldn't be read: {e}"),
                )
            }
        };

        let messages: Vec<Value> = if event_stream {
            parse_event_stream(&body)
        } else {
            serde_json::from_str::<Value>(&body)
                .ok()
                .into_iter()
                .collect()
        };
        if messages.is_empty() {
            return if status.is_success() {
                Vec::new() // 202: a notification was accepted, and has no answer.
            } else {
                answer(INTERNAL_ERROR, &format!("The workspace answered {status}."))
            };
        }

        if message.get("method").and_then(Value::as_str) == Some("initialize") {
            self.negotiated = messages
                .iter()
                .find_map(|m| m.pointer("/result/protocolVersion").and_then(Value::as_str))
                .filter(|version| *version < FIRST_PER_REQUEST_VERSION)
                .map(str::to_string);
        }
        // Serialised again so each message is exactly one line.
        messages.iter().map(Value::to_string).collect()
    }

    async fn post(&self, body: &str, message: &Value) -> Result<reqwest::Response, String> {
        let token = self.connections.access_token(&self.connection_id).await?;
        let mut request = self
            .http
            .post(&self.endpoint)
            .bearer_auth(token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            );
        for (name, value) in protocol_headers(message, self.negotiated.as_deref()) {
            request = request.header(name, value);
        }
        request
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| format!("The workspace couldn't be reached: {e}"))
    }
}

/// The headers MCP mirrors from a message. From 2026-07-28 a request declares
/// its version in `_meta`, with the method and target repeated in headers;
/// before that, the version from `initialize` is the header.
fn protocol_headers(message: &Value, negotiated: Option<&str>) -> Vec<(&'static str, String)> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params");
    let declared = params
        .and_then(|p| p.get("_meta"))
        .and_then(|m| m.get(META_VERSION))
        .and_then(Value::as_str);
    let Some(version) = declared else {
        return negotiated
            .map(|v| vec![("MCP-Protocol-Version", v.to_string())])
            .unwrap_or_default();
    };

    let mut headers = vec![
        ("MCP-Protocol-Version", version.to_string()),
        ("Mcp-Method", method.to_string()),
    ];
    let target = match method {
        "tools/call" | "prompts/get" => params.and_then(|p| p.get("name")),
        "resources/read" => params.and_then(|p| p.get("uri")),
        _ => None,
    };
    if let Some(target) = target.and_then(Value::as_str) {
        // Encoded, since a name or URI need not be valid in a header.
        headers.push((
            "Mcp-Name",
            format!("=?base64?{}?=", STANDARD.encode(target)),
        ));
    }
    headers
}

/// The JSON messages in a complete `text/event-stream` body.
fn parse_event_stream(body: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut data = String::new();
    for line in body.lines().chain(std::iter::once("")) {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.is_empty() && !data.is_empty() {
            if let Ok(value) = serde_json::from_str(&data) {
                messages.push(value);
            }
            data.clear();
        }
    }
    messages
}

/// The answer to a failed relay. The sign-in being gone or refused needs the
/// person; anything else may pass.
fn failed(reason: &str, answer: impl Fn(i64, &str) -> Vec<String>) -> Vec<String> {
    if reason == "not signed in" || reason.contains("invalid_grant") {
        answer(NOT_SIGNED_IN, SIGN_IN_AGAIN)
    } else {
        answer(INTERNAL_ERROR, reason)
    }
}

fn error_line(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;
    use crate::store::{keychain_account, Connection, SecretStore, State, StateFile};
    use std::sync::{Arc, Mutex};
    use std::thread;

    type Seen = (String, Vec<(String, String)>);

    /// A workspace and its authorization server: records what the MCP endpoint
    /// received and the refresh posts, and refuses a token in `reject` with a 401.
    struct Mock {
        origin: String,
        seen: Arc<Mutex<Vec<Seen>>>,
        reject: Arc<Mutex<Vec<String>>>,
        refreshes: Arc<Mutex<Vec<String>>>,
    }

    /// One keychain, as separate processes share it.
    struct SharedStore(Arc<MemoryStore>);

    impl SecretStore for SharedStore {
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            self.0.set(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            self.0.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.delete(account)
        }
    }

    fn mock(answer: fn(&str) -> (u16, &'static str, String)) -> Mock {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!(
            "http://127.0.0.1:{}",
            server.server_addr().to_ip().unwrap().port()
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reject: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let refreshes = Arc::new(Mutex::new(Vec::new()));
        let (s, r, f) = (seen.clone(), reject.clone(), refreshes.clone());
        thread::spawn(move || {
            let mut issued = 0;
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let (status, kind, text) = if request.url() == "/oauth/token" {
                    issued += 1;
                    f.lock().unwrap().push(body.clone());
                    (200, "application/json", json!({ "access_token": format!("at-{issued}"), "refresh_token": format!("rt-{issued}"), "token_type": "Bearer", "expires_in": 3600 }).to_string())
                } else {
                    let headers: Vec<(String, String)> = request
                        .headers()
                        .iter()
                        .map(|h| (h.field.to_string(), h.value.to_string()))
                        .collect();
                    let bearer = headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    s.lock().unwrap().push((body.clone(), headers));
                    if r.lock()
                        .unwrap()
                        .iter()
                        .any(|t| bearer.ends_with(t.as_str()))
                    {
                        (401, "application/json", String::new())
                    } else {
                        answer(&body)
                    }
                };
                let header = tiny_http::Header::from_bytes("Content-Type", kind).unwrap();
                let _ = request.respond(
                    tiny_http::Response::from_string(text)
                        .with_status_code(status)
                        .with_header(header),
                );
            }
        });
        Mock {
            origin,
            seen,
            reject,
            refreshes,
        }
    }

    fn signed_in(origin: &str) -> (Connections, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "whity-agent-bridge-{}",
            crate::oauth::random_token()
        ));
        let file = StateFile::new(&dir);
        let state = State {
            connections: vec![Connection {
                id: "c1".into(),
                resource: format!("{origin}/mcp"),
                name: None,
                issuer: origin.into(),
                token_endpoint: format!("{origin}/oauth/token"),
                client_id: "client-1".into(),
            }],
            ..State::default()
        };
        file.save(&state).unwrap();
        let secrets = MemoryStore::default();
        secrets.set(&keychain_account("c1"), "rt-0").unwrap();
        (
            Connections::new(StateFile::new(&dir), Box::new(secrets)),
            dir,
        )
    }

    fn echo(body: &str) -> (u16, &'static str, String) {
        let id = serde_json::from_str::<Value>(body).unwrap()["id"].clone();
        (
            200,
            "application/json",
            json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": "2025-06-18" } })
                .to_string(),
        )
    }

    fn header(seen: &(String, Vec<(String, String)>), name: &str) -> Option<String> {
        seen.1
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    #[tokio::test]
    async fn it_relays_a_request_with_the_keychain_token_and_one_line_back() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .await;

        assert_eq!(out.len(), 1);
        assert!(!out[0].contains('\n'));
        assert_eq!(serde_json::from_str::<Value>(&out[0]).unwrap()["id"], 1);
        let seen = server.seen.lock().unwrap();
        assert_eq!(
            header(&seen[0], "authorization").as_deref(),
            Some("Bearer at-1")
        );
        assert!(!out[0].contains("at-1"), "the token never reaches the tool");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_loop_answers_line_by_line_until_input_closes() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));
        // Two messages with a blank line between them, which is skipped.
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}

{"jsonrpc":"2.0","id":2,"method":"ping"}
"#;
        let mut output = Vec::new();

        bridge.relay(&input[..], &mut output).await.unwrap();

        let output = String::from_utf8(output).unwrap();
        let ids: Vec<Value> = output
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["id"].clone())
            .collect();
        assert_eq!(ids, vec![json!(1), json!(2)]);
        assert!(output.ends_with('\n'));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_negotiated_legacy_version_goes_in_the_header_afterwards() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));

        bridge.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#).await;
        bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await;

        let seen = server.seen.lock().unwrap();
        assert_eq!(
            header(&seen[0], "mcp-protocol-version"),
            None,
            "initialize negotiates it"
        );
        assert_eq!(
            header(&seen[1], "mcp-protocol-version").as_deref(),
            Some("2025-06-18")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_per_request_version_is_mirrored_with_the_method_and_target() {
        let message = json!({ "method": "tools/call", "params": { "name": "get_api_me", "_meta": { META_VERSION: "2026-07-28" } } });
        let headers = protocol_headers(&message, Some("2025-06-18"));

        assert!(
            headers.contains(&("MCP-Protocol-Version", "2026-07-28".to_string())),
            "the request's own wins"
        );
        assert!(headers.contains(&("Mcp-Method", "tools/call".to_string())));
        assert!(headers.contains(&("Mcp-Name", "=?base64?Z2V0X2FwaV9tZQ==?=".to_string())));
    }

    #[tokio::test]
    async fn a_401_refreshes_once_and_retries() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));
        bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .await;
        server.reject.lock().unwrap().push("at-1".into());

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#)
            .await;

        assert_eq!(serde_json::from_str::<Value>(&out[0]).unwrap()["id"], 2);
        let seen = server.seen.lock().unwrap();
        assert_eq!(
            header(seen.last().unwrap(), "authorization").as_deref(),
            Some("Bearer at-2")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn still_refused_says_to_sign_in_again_and_a_notification_stays_silent() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));
        for token in ["at-1", "at-2", "at-3", "at-4"] {
            server.reject.lock().unwrap().push(token.into());
        }

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)
            .await;
        let answer: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["error"]["code"], NOT_SIGNED_IN);
        assert!(answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("sign in again"));

        assert!(bridge
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_accepted_notification_has_no_answer_and_bad_json_gets_a_parse_error() {
        let server = mock(|_| (202, "application/json", String::new()));
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));

        assert!(bridge
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .is_empty());

        let bad: Value = serde_json::from_str(&bridge.handle_line("{nope").await[0]).unwrap();
        assert_eq!(bad["error"]["code"], PARSE_ERROR);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_event_stream_becomes_one_line_per_message() {
        let server = mock(|_| {
            (200, "text/event-stream", "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n".to_string())
        });
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .await;

        assert_eq!(out.len(), 2);
        assert!(out[0].contains("list_changed"));
        assert_eq!(serde_json::from_str::<Value>(&out[1]).unwrap()["id"], 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_unreachable_workspace_answers_with_an_error_not_silence() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        let mut bridge = Bridge::new(&connections, "c1", "http://127.0.0.1:9/mcp");

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#)
            .await;
        let answer: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(answer["id"], 3);
        assert_eq!(answer["error"]["code"], INTERNAL_ERROR);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_sign_in_that_is_gone_says_to_sign_in_again() {
        let server = mock(echo);
        let (connections, dir) = signed_in(&server.origin);
        connections.sign_out("c1").unwrap();
        let mut bridge = Bridge::new(&connections, "c1", &format!("{}/mcp", server.origin));

        let out = bridge
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#)
            .await;
        let answer: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(answer["error"]["code"], NOT_SIGNED_IN);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn two_processes_refreshing_never_reuse_a_rotated_token() {
        let server = mock(echo);
        let (_, dir) = signed_in(&server.origin);
        let keychain = Arc::new(MemoryStore::default());
        keychain.set(&keychain_account("c1"), "rt-0").unwrap();
        let process = || {
            Connections::new(
                StateFile::new(&dir),
                Box::new(SharedStore(keychain.clone())),
            )
        };
        let (one, two) = (process(), process());

        let (a, b) = tokio::join!(one.access_token("c1"), two.access_token("c1"));
        a.unwrap();
        b.unwrap();

        let posts = server.refreshes.lock().unwrap();
        assert_eq!(posts.len(), 2);
        assert!(posts[0].contains("refresh_token=rt-0"), "{}", posts[0]);
        assert!(posts[1].contains("refresh_token=rt-1"), "{}", posts[1]);
        assert_eq!(
            keychain.get(&keychain_account("c1")).unwrap().as_deref(),
            Some("rt-2")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
