//! Signed-in workspaces: signing in, listing and signing out (WHIT-822).
//!
//! Access tokens live in memory only. A refresh token, when the server gives
//! one, goes to the OS keychain and the connection is remembered; if the
//! keychain is unavailable, the connection lasts until the app closes and
//! nothing secret is written anywhere.

use crate::loopback::{Loopback, SIGN_IN_WINDOW};
use crate::oauth::{self, Pkce, Tokens};
use crate::store::{connection_id, keychain_account, Connection, SecretStore, State, StateFile};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use url::Url;

/// Held while refreshing a token, across every whity-agent process. A lock file
/// left by a crash stops counting after [`RefreshLock::STALE`].
struct RefreshLock {
    path: PathBuf,
}

impl RefreshLock {
    const STALE: Duration = Duration::from_secs(30);

    async fn acquire(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = dir.join("refresh.lock");
        let deadline = Instant::now() + Self::STALE + Duration::from_secs(5);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|at| at.elapsed().ok())
                        .is_some_and(|age| age > Self::STALE);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                    } else if Instant::now() > deadline {
                        return Err("another Whity Agent is still refreshing the sign-in".into());
                    } else {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                Err(e) => return Err(format!("can't take the sign-in lock: {e}")),
            }
        }
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What the webview sees of a connection (src/api.ts). No tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionView {
    pub id: String,
    pub resource: String,
    pub name: Option<String>,
    pub issuer: String,
    pub persisted: bool,
    pub agent: Option<String>,
}

pub struct Connections {
    http: reqwest::Client,
    file: StateFile,
    secrets: Box<dyn SecretStore>,
    state: Mutex<State>,
    /// Connections that last until the app closes (no refresh token, or no keychain).
    session_only: Mutex<Vec<Connection>>,
    /// Access tokens and when each stops being usable, by connection id. Never
    /// written anywhere.
    access: Mutex<HashMap<String, (Tokens, Option<Instant>)>>,
    window: Duration,
}

impl Connections {
    pub fn new(file: StateFile, secrets: Box<dyn SecretStore>) -> Self {
        let state = file.load();
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("whity-agent/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("an HTTP client"),
            file,
            secrets,
            state: Mutex::new(state),
            session_only: Mutex::new(Vec::new()),
            access: Mutex::new(HashMap::new()),
            window: SIGN_IN_WINDOW,
        }
    }

    pub fn list(&self) -> Vec<ConnectionView> {
        let saved = self.state.lock().unwrap().connections.clone();
        let session = self.session_only.lock().unwrap().clone();
        saved
            .iter()
            .map(|c| view(c, true))
            .chain(
                session
                    .iter()
                    .filter(|c| !saved.iter().any(|s| s.id == c.id))
                    .map(|c| view(c, false)),
            )
            .collect()
    }

    /// The whole sign-in. `open` shows the person the authorization URL in
    /// their browser; tests pass a fake browser instead.
    pub async fn sign_in<F>(&self, address: &str, open: F) -> Result<ConnectionView, String>
    where
        F: FnOnce(&Url) -> Result<(), String>,
    {
        let origin = oauth::normalize_address(address)?;
        self.sign_in_to(&origin, "mcp", None, open).await
    }

    /// Sign in again as one of the workspace's agents, which is its own resource
    /// with its own token and grant. The person's own sign-in stays as it is.
    pub async fn sign_in_agent<F>(
        &self,
        connection: &str,
        slug: &str,
        open: F,
    ) -> Result<ConnectionView, String>
    where
        F: FnOnce(&Url) -> Result<(), String>,
    {
        if !oauth::is_agent_slug(slug) {
            return Err("that isn't an agent's name".into());
        }
        let resource = self
            .list()
            .into_iter()
            .find(|c| c.id == connection && c.agent.is_none())
            .ok_or("not signed in to that workspace")?
            .resource;
        let origin = Url::parse(&resource)
            .and_then(|u| u.join("/"))
            .map_err(|e| e.to_string())?;
        self.sign_in_to(&origin, &format!("mcp/agents/{slug}"), Some(slug), open)
            .await
    }

    async fn sign_in_to<F>(
        &self,
        origin: &Url,
        path: &str,
        agent: Option<&str>,
        open: F,
    ) -> Result<ConnectionView, String>
    where
        F: FnOnce(&Url) -> Result<(), String>,
    {
        let discovery = oauth::discover(&self.http, origin, path).await?;
        let client_id = self.client_id_for(&discovery).await?;

        let pkce = Pkce::new();
        let state = oauth::random_token();
        let loopback = Loopback::bind().await?;
        let redirect_uri = loopback.redirect_uri.clone();
        open(&oauth::authorize_url(
            &discovery,
            &client_id,
            &redirect_uri,
            &pkce,
            &state,
        )?)?;

        let target = loopback.receive(self.window).await?;
        let code = oauth::parse_callback(&target, &state, &discovery.issuer)?;
        let tokens = match oauth::exchange_code(
            &self.http,
            &discovery,
            &client_id,
            &redirect_uri,
            &code,
            &pkce,
        )
        .await
        {
            Ok(tokens) => tokens,
            Err(e) => {
                if e.contains("invalid_client") {
                    // The server forgot our registration; register afresh next time.
                    let mut state = self.state.lock().unwrap();
                    state.clients.remove(&discovery.issuer);
                    let _ = self.file.save(&state);
                }
                return Err(e);
            }
        };

        let connection = Connection {
            id: connection_id(&discovery.resource),
            resource: discovery.resource.clone(),
            name: discovery.resource_name.clone(),
            issuer: discovery.issuer.clone(),
            token_endpoint: discovery.token_endpoint.clone(),
            client_id,
            agent: agent.map(str::to_string),
        };
        let persisted = match tokens.refresh_token.as_deref() {
            Some(refresh) => self
                .secrets
                .set(&keychain_account(&connection.id), refresh)
                .is_ok(),
            None => false,
        };

        if persisted {
            let mut state = self.state.lock().unwrap();
            state.connections.retain(|c| c.id != connection.id);
            state.connections.push(connection.clone());
            self.file.save(&state)?;
            self.session_only
                .lock()
                .unwrap()
                .retain(|c| c.id != connection.id);
        } else {
            let mut session = self.session_only.lock().unwrap();
            session.retain(|c| c.id != connection.id);
            session.push(connection.clone());
        }
        self.remember(&connection.id, tokens);

        Ok(view(&connection, persisted))
    }

    /// A copy of the saved state, for listing tools and their links.
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Change the saved state and write it, under the lock (no awaiting inside).
    pub fn update_state<R>(&self, change: impl FnOnce(&mut State) -> R) -> Result<R, String> {
        let mut state = self.state.lock().unwrap();
        let result = change(&mut state);
        self.file.save(&state)?;
        Ok(result)
    }

    /// What to name a signed-in workspace in a tool, its MCP URL, and whether its
    /// sign-in is kept in the keychain. Tools name the server after the host,
    /// which, unlike the workspace's own name, is unique, and the agent if any.
    pub fn workspace(&self, id: &str) -> Option<(String, String, bool)> {
        self.list().into_iter().find(|c| c.id == id).map(|c| {
            let host = url::Url::parse(&c.resource)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            let name = match c.agent {
                Some(agent) => format!("{host} {agent}"),
                None => host,
            };
            (name, c.resource, c.persisted)
        })
    }

    pub fn sign_out(&self, id: &str) -> Result<(), String> {
        self.access.lock().unwrap().remove(id);
        self.session_only.lock().unwrap().retain(|c| c.id != id);
        let mut state = self.state.lock().unwrap();
        if state.connections.iter().any(|c| c.id == id) {
            state.connections.retain(|c| c.id != id);
            self.file.save(&state)?;
        }
        self.secrets.delete(&keychain_account(id))
    }

    /// A fresh access token for a connection: the one in memory, or a new one
    /// from the refresh token in the keychain. Rotated refresh tokens are
    /// stored back. For the MCP bridge and the chat (WHIT-824).
    ///
    /// Refresh tokens rotate, and the authorization server treats a reused one
    /// as theft and revokes the whole chain. The app and any number of bridges
    /// share one, so refreshing holds a lock across processes, and reads the
    /// keychain inside it: whoever goes second uses what the first stored.
    pub async fn access_token(&self, id: &str) -> Result<String, String> {
        if let Some((tokens, expires)) = self.access.lock().unwrap().get(id) {
            if expires.is_none_or(|at| at > Instant::now()) {
                return Ok(tokens.access_token.clone());
            }
        }
        let connection = self
            .state
            .lock()
            .unwrap()
            .connections
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or("not signed in")?;
        let _lock = RefreshLock::acquire(self.file.dir()).await?;
        let account = keychain_account(id);
        let refresh = self.secrets.get(&account)?.ok_or("not signed in")?;
        let tokens = oauth::refresh(
            &self.http,
            &connection.token_endpoint,
            &connection.client_id,
            &connection.resource,
            &refresh,
        )
        .await?;
        if let Some(rotated) = tokens.refresh_token.as_deref() {
            self.secrets.set(&account, rotated)?;
        }
        let access = tokens.access_token.clone();
        self.remember(id, tokens);
        Ok(access)
    }

    /// Drop a cached access token the server refused, so the next use refreshes.
    pub fn forget_access_token(&self, id: &str) {
        self.access.lock().unwrap().remove(id);
    }

    /// Keep an access token until a minute before it expires.
    fn remember(&self, id: &str, tokens: Tokens) {
        let expires = tokens
            .expires_in
            .map(|seconds| Instant::now() + Duration::from_secs(seconds.saturating_sub(60)));
        self.access
            .lock()
            .unwrap()
            .insert(id.to_string(), (tokens, expires));
    }

    /// The client id to sign in with: whity-agent's published client metadata
    /// document where the authorization server knows it (Whity ID), else a
    /// public client registered there once (RFC 7591), for self-hosted servers.
    async fn client_id_for(&self, discovery: &oauth::Discovery) -> Result<String, String> {
        if let Some(id) = oauth::metadata_document_client_id(&discovery.issuer) {
            return Ok(id.to_string());
        }
        if let Some(id) = self.state.lock().unwrap().clients.get(&discovery.issuer) {
            return Ok(id.clone());
        }
        let endpoint = discovery
            .registration_endpoint
            .as_deref()
            .ok_or(oauth::NO_SIGN_IN)?;
        let client_id = oauth::register_client(&self.http, endpoint).await?;
        let mut state = self.state.lock().unwrap();
        state
            .clients
            .insert(discovery.issuer.clone(), client_id.clone());
        self.file.save(&state)?;
        Ok(client_id)
    }

    #[cfg(test)]
    fn with_window(mut self, window: Duration) -> Self {
        self.window = window;
        self
    }
}

fn view(connection: &Connection, persisted: bool) -> ConnectionView {
    ConnectionView {
        id: connection.id.clone(),
        resource: connection.resource.clone(),
        name: connection.name.clone(),
        issuer: connection.issuer.clone(),
        persisted,
        agent: connection.agent.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::memory::MemoryStore;
    use std::sync::Arc;
    use std::thread;

    /// A workspace and its authorization server on one local port, answering
    /// the requests a sign-in makes and recording the form posts it receives.
    struct MockServer {
        origin: String,
        posts: Arc<Mutex<Vec<(String, String)>>>,
        registrations: Arc<Mutex<usize>>,
    }

    fn mock(refresh_token: Option<&'static str>) -> MockServer {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!(
            "http://127.0.0.1:{}",
            server.server_addr().to_ip().unwrap().port()
        );
        let posts = Arc::new(Mutex::new(Vec::new()));
        let registrations = Arc::new(Mutex::new(0));
        let (o, p, r) = (origin.clone(), posts.clone(), registrations.clone());
        thread::spawn(move || {
            for mut request in server.incoming_requests() {
                let mut body = String::new();
                let _ = request.as_reader().read_to_string(&mut body);
                let url = request.url().to_string();
                let json = match url.as_str() {
                    "/.well-known/oauth-protected-resource/mcp/agents/finance-clerk" => {
                        serde_json::json!({
                            "resource": format!("{o}/mcp/agents/finance-clerk"),
                            "authorization_servers": [o.clone()],
                            "scopes_supported": ["tools:call"],
                            "resource_name": "Finance clerk",
                        })
                    }
                    "/.well-known/oauth-protected-resource/mcp/agents/lying" => serde_json::json!({
                        "resource": format!("{o}/mcp"),
                        "authorization_servers": [o.clone()],
                    }),
                    "/.well-known/oauth-protected-resource/mcp" => serde_json::json!({
                        "resource": format!("{o}/mcp"),
                        "authorization_servers": [o.clone()],
                        "scopes_supported": ["tools:call", "resources:read", "prompts:read"],
                        "resource_name": "Example Bakery",
                    }),
                    "/.well-known/oauth-authorization-server" => serde_json::json!({
                        "issuer": o.clone(),
                        "authorization_endpoint": format!("{o}/authorize"),
                        "token_endpoint": format!("{o}/oauth/token"),
                        "registration_endpoint": format!("{o}/oauth/register"),
                        "code_challenge_methods_supported": ["S256"],
                    }),
                    "/oauth/register" => {
                        *r.lock().unwrap() += 1;
                        serde_json::json!({ "client_id": "client-1" })
                    }
                    "/oauth/token" => {
                        p.lock().unwrap().push((url.clone(), body.clone()));
                        let mut tokens = serde_json::json!({ "access_token": "at-1", "token_type": "Bearer", "expires_in": 3600 });
                        if let Some(rt) = refresh_token {
                            tokens["refresh_token"] = serde_json::json!(rt);
                        }
                        tokens
                    }
                    _ => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                        continue;
                    }
                };
                let header =
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap();
                let _ = request.respond(
                    tiny_http::Response::from_string(json.to_string()).with_header(header),
                );
            }
        });
        MockServer {
            origin,
            posts,
            registrations,
        }
    }

    /// A browser that signs in at once: it reads the authorize URL and calls
    /// the loopback redirect with a code and the same state.
    fn browser(issuer: String) -> impl FnOnce(&Url) -> Result<(), String> {
        move |url: &Url| {
            let get = |k: &str| {
                url.query_pairs()
                    .find(|(key, _)| key == k)
                    .map(|(_, v)| v.into_owned())
                    .unwrap()
            };
            let callback = format!(
                "{}?code=code-1&state={}&iss={}",
                get("redirect_uri"),
                get("state"),
                url::form_urlencoded::byte_serialize(issuer.as_bytes()).collect::<String>()
            );
            tokio::spawn(async move {
                let _ = reqwest::get(callback).await;
            });
            Ok(())
        }
    }

    fn connections(secrets: MemoryStore) -> (Connections, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("whity-agent-test-{}", oauth::random_token()));
        (
            Connections::new(StateFile::new(&dir), Box::new(secrets))
                .with_window(Duration::from_secs(10)),
            dir,
        )
    }

    #[tokio::test]
    async fn signing_in_keeps_the_refresh_token_in_the_keychain_only() {
        let server = mock(Some("rt-1"));
        let (agent, dir) = connections(MemoryStore::default());

        let view = agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();

        assert!(view.persisted);
        assert_eq!(view.name.as_deref(), Some("Example Bakery"));
        assert_eq!(view.resource, format!("{}/mcp", server.origin));
        // PKCE and the resource went with the code.
        let (_, form) = server.posts.lock().unwrap()[0].clone();
        assert!(
            form.contains("grant_type=authorization_code")
                && form.contains("code_verifier=")
                && form.contains("resource=")
        );
        // The state file names the connection and holds no token.
        let raw = std::fs::read_to_string(dir.join("state.json")).unwrap();
        assert!(raw.contains("client-1") && !raw.contains("rt-1") && !raw.contains("at-1"));
        assert_eq!(agent.access_token(&view.id).await.unwrap(), "at-1");

        // Signing in again reuses the registration and the connection.
        agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();
        assert_eq!(*server.registrations.lock().unwrap(), 1);
        assert_eq!(agent.list().len(), 1);

        agent.sign_out(&view.id).unwrap();
        assert!(agent.list().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_a_keychain_the_sign_in_lasts_until_the_app_closes() {
        let server = mock(Some("rt-1"));
        let (agent, dir) = connections(MemoryStore {
            broken: true,
            ..Default::default()
        });

        let view = agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();

        assert!(!view.persisted);
        assert_eq!(agent.list(), vec![view]);
        assert!(
            StateFile::new(&dir).load().connections.is_empty(),
            "not remembered across restarts"
        );
        let raw = std::fs::read_to_string(dir.join("state.json")).unwrap_or_default();
        assert!(!raw.contains("rt-1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_refresh_token_brings_back_an_access_token_after_a_restart() {
        let server = mock(Some("rt-1"));
        let (agent, dir) = connections(MemoryStore::default());
        let view = agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();
        let secrets = MemoryStore::default();
        secrets.set(&keychain_account(&view.id), "rt-1").unwrap();

        let restarted = Connections::new(StateFile::new(&dir), Box::new(secrets));
        assert_eq!(restarted.access_token(&view.id).await.unwrap(), "at-1");
        let (_, form) = server.posts.lock().unwrap().last().cloned().unwrap();
        assert!(form.contains("grant_type=refresh_token") && form.contains("refresh_token=rt-1"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_lock_left_by_a_crash_stops_counting() {
        let dir = std::env::temp_dir().join(format!("whity-agent-test-{}", oauth::random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let left = std::fs::File::create(dir.join("refresh.lock")).unwrap();
        left.set_modified(std::time::SystemTime::now() - Duration::from_secs(120))
            .unwrap();
        drop(left);

        let lock = RefreshLock::acquire(&dir).await.unwrap();
        drop(lock);
        assert!(!dir.join("refresh.lock").exists(), "released when dropped");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_agent_is_its_own_sign_in_beside_the_persons() {
        let server = mock(Some("rt-1"));
        let (agent, dir) = connections(MemoryStore::default());
        let person = agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();

        let clerk = agent
            .sign_in_agent(&person.id, "finance-clerk", browser(server.origin.clone()))
            .await
            .unwrap();

        assert_ne!(clerk.id, person.id);
        assert_eq!(clerk.agent.as_deref(), Some("finance-clerk"));
        assert_eq!(
            clerk.resource,
            format!("{}/mcp/agents/finance-clerk", server.origin)
        );
        assert_eq!(agent.list().len(), 2);
        let (_, form) = server.posts.lock().unwrap().last().cloned().unwrap();
        assert!(form.contains("mcp%2Fagents%2Ffinance-clerk"), "{form}");
        // Tools get a name of its own for it.
        let (name, _, _) = agent.workspace(&clerk.id).unwrap();
        assert!(name.ends_with(" finance-clerk"), "{name}");
        // An agent can't be the way in to another agent.
        assert!(agent
            .sign_in_agent(&clerk.id, "finance-clerk", browser(server.origin.clone()))
            .await
            .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_agent_sign_in_refuses_a_bad_name_or_another_resource() {
        let server = mock(Some("rt-1"));
        let (agent, dir) = connections(MemoryStore::default());
        let person = agent
            .sign_in(&server.origin, browser(server.origin.clone()))
            .await
            .unwrap();

        for bad in ["../mcp", "Finance", "", "-x", "a/b"] {
            assert!(
                agent
                    .sign_in_agent(&person.id, bad, |_| Ok(()))
                    .await
                    .is_err(),
                "{bad}"
            );
        }
        let error = agent
            .sign_in_agent(&person.id, "lying", |_| Ok(()))
            .await
            .unwrap_err();
        assert!(error.contains("another resource"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_workspace_without_sign_in_says_so() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let origin = format!(
            "http://127.0.0.1:{}",
            server.server_addr().to_ip().unwrap().port()
        );
        thread::spawn(move || {
            for request in server.incoming_requests() {
                let _ = request.respond(tiny_http::Response::empty(404));
            }
        });
        let (agent, _) = connections(MemoryStore::default());
        assert_eq!(
            agent.sign_in(&origin, |_| Ok(())).await.unwrap_err(),
            oauth::NO_SIGN_IN
        );
    }
}
