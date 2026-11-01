//! Signing in to a Whity workspace (WHIT-822): OAuth 2.1 authorization code
//! with PKCE, in the system browser, with a loopback redirect (RFC 8252).
//!
//! 1. The workspace says which authorization server it trusts, in its
//!    Protected Resource Metadata (RFC 9728) for `/mcp`.
//! 2. The authorization server's metadata (RFC 8414) gives the endpoints; its
//!    `issuer` must be the one the workspace named.
//! 3. whity-agent registers itself as a public client (RFC 7591) once per
//!    authorization server, unless it already has a client id there.
//! 4. The browser signs the person in; the code comes back to a one-shot
//!    listener on 127.0.0.1, checked against `state` and `iss` (RFC 9207).
//! 5. The code is exchanged for tokens bound to the workspace's MCP resource
//!    (RFC 8707).
//!
//! Everything here is plain functions over `reqwest`, so the tests drive it
//! against a local mock server.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

/// Error codes the UI translates (src/App.tsx); any other error is a reason in words.
pub const INVALID_ADDRESS: &str = "invalid_address";
pub const NO_SIGN_IN: &str = "no_sign_in";
pub const CANCELLED: &str = "cancelled";

pub const CLIENT_NAME: &str = "Whity Agent";

/// whity-agent's Client ID Metadata Documents, by issuer: Whity ID knows the
/// app as a first-party public client (PKCE S256, loopback `/callback` on any
/// port), so no registration is needed there.
const METADATA_DOCUMENT_CLIENTS: &[(&str, &str)] = &[(
    "https://id.whity.dev",
    "https://id.whity.dev/clients/whity-agent.json",
)];

/// The client id to use at an authorization server that publishes
/// whity-agent's metadata document, if this is one.
pub fn metadata_document_client_id(issuer: &str) -> Option<&'static str> {
    let issuer = issuer.trim_end_matches('/');
    METADATA_DOCUMENT_CLIENTS
        .iter()
        .find(|(known, _)| *known == issuer)
        .map(|(_, client_id)| *client_id)
}

/// What a workspace and its authorization server say about signing in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub resource: String,
    pub resource_name: Option<String>,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds the access token lives, when the server says.
    pub expires_in: Option<u64>,
}

/// Turn what a person types ("app.whity.dev", "https://app.whity.dev/home")
/// into the workspace's origin. https only, except a loopback address, so a
/// local development instance can be signed in to.
pub fn normalize_address(input: &str) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(INVALID_ADDRESS.into());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&with_scheme).map_err(|_| INVALID_ADDRESS.to_string())?;
    let host = url.host_str().ok_or(INVALID_ADDRESS)?.to_string();
    let allowed = match url.scheme() {
        "https" => true,
        "http" => is_loopback(&host),
        _ => false,
    };
    if !allowed || !url.username().is_empty() || url.password().is_some() {
        return Err(INVALID_ADDRESS.into());
    }
    let mut origin = url;
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    Ok(origin)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".localhost")
}

/// A workspace agent's slug, as core's router takes it: it goes into a URL.
pub fn is_agent_slug(slug: &str) -> bool {
    slug.len() <= 48
        && slug.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An endpoint the metadata names must be https, or loopback http.
fn checked_endpoint(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| format!("bad endpoint {value}"))?;
    let ok = url.scheme() == "https"
        || (url.scheme() == "http" && url.host_str().is_some_and(is_loopback));
    if ok {
        Ok(value.to_string())
    } else {
        Err(format!("endpoint is not https: {value}"))
    }
}

#[derive(Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
    resource_name: Option<String>,
}

#[derive(Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: Option<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

/// Steps 1 and 2: who signs people in for this workspace's MCP resource at
/// `path` (`mcp`, or `mcp/agents/<slug>`), and where.
pub async fn discover(
    http: &reqwest::Client,
    origin: &Url,
    path: &str,
) -> Result<Discovery, String> {
    let prm_url = origin
        .join(&format!(".well-known/oauth-protected-resource/{path}"))
        .map_err(|e| e.to_string())?;
    let response = http.get(prm_url).send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(NO_SIGN_IN.into());
    }
    let prm: ProtectedResourceMetadata =
        response.json().await.map_err(|_| NO_SIGN_IN.to_string())?;
    let issuer = prm
        .authorization_servers
        .first()
        .cloned()
        .ok_or(NO_SIGN_IN)?;
    // The resource must be this workspace's own, or a token for it would be
    // a token for someone else.
    let resource = Url::parse(&prm.resource).map_err(|_| NO_SIGN_IN.to_string())?;
    if resource.origin() != origin.origin() || !resource.path().ends_with(&format!("/{path}")) {
        return Err(format!(
            "the workspace named another resource: {}",
            prm.resource
        ));
    }

    let issuer_url = Url::parse(&checked_endpoint(&issuer)?).map_err(|e| e.to_string())?;
    let asm_url = issuer_url
        .join(".well-known/oauth-authorization-server")
        .map_err(|e| e.to_string())?;
    let asm: AuthorizationServerMetadata = http
        .get(asm_url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if asm.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(format!("issuer mismatch: {} is not {}", asm.issuer, issuer));
    }
    if !asm
        .code_challenge_methods_supported
        .iter()
        .any(|m| m == "S256")
    {
        return Err("the sign-in server doesn't support PKCE S256".into());
    }

    Ok(Discovery {
        resource: prm.resource,
        resource_name: prm.resource_name,
        issuer: asm.issuer,
        authorization_endpoint: checked_endpoint(&asm.authorization_endpoint)?,
        token_endpoint: checked_endpoint(&asm.token_endpoint)?,
        registration_endpoint: asm
            .registration_endpoint
            .as_deref()
            .map(checked_endpoint)
            .transpose()?,
        scopes: prm.scopes_supported,
    })
}

/// A PKCE pair (RFC 7636): the verifier stays here, the S256 challenge goes out.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// 32 random bytes, base64url: 43 characters, enough for a verifier or a state.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Step 3: register as a public client with a loopback redirect. The port is
/// left out: RFC 8252 §7.3 has the server accept any port on loopback.
pub async fn register_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Registered {
        client_id: String,
    }
    let body = serde_json::json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": ["http://127.0.0.1/callback"],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "client_uri": "https://github.com/whity-org/whity-agent",
    });
    let response = http
        .post(registration_endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "client registration failed ({})",
            response.status()
        ));
    }
    let registered: Registered = response.json().await.map_err(|e| e.to_string())?;
    Ok(registered.client_id)
}

/// The browser URL for step 4.
pub fn authorize_url(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    pkce: &Pkce,
    state: &str,
) -> Result<Url, String> {
    let mut url = Url::parse(&discovery.authorization_endpoint).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("resource", &discovery.resource)
        .append_pair("scope", &discovery.scopes.join(" "));
    Ok(url)
}

/// Read the code from the loopback request's target (`/callback?code=…`),
/// checking `state` and, when the server sends it, `iss` (RFC 9207).
pub fn parse_callback(
    target: &str,
    expected_state: &str,
    expected_issuer: &str,
) -> Result<String, String> {
    let url =
        Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| "bad callback".to_string())?;
    if url.path() != "/callback" {
        return Err("bad callback".into());
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    };

    if param("state").as_deref() != Some(expected_state) {
        return Err("state mismatch".into());
    }
    if let Some(iss) = param("iss") {
        if iss.trim_end_matches('/') != expected_issuer.trim_end_matches('/') {
            return Err("issuer mismatch in the callback".into());
        }
    }
    if let Some(error) = param("error") {
        return Err(if error == "access_denied" {
            CANCELLED.into()
        } else {
            error
        });
    }
    param("code")
        .filter(|c| !c.is_empty())
        .ok_or_else(|| "no code in the callback".into())
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[serde(default)]
    token_type: String,
}

async fn token_request(
    http: &reqwest::Client,
    token_endpoint: &str,
    form: &[(&str, &str)],
) -> Result<Tokens, String> {
    let response = http
        .post(token_endpoint)
        .form(form)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let error = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        return Err(format!("token request failed: {error}"));
    }
    let tokens: TokenResponse = response.json().await.map_err(|e| e.to_string())?;
    if !tokens.token_type.eq_ignore_ascii_case("bearer") {
        return Err(format!("unexpected token type {}", tokens.token_type));
    }
    Ok(Tokens {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_in: tokens.expires_in,
    })
}

/// Step 5.
pub async fn exchange_code(
    http: &reqwest::Client,
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    pkce: &Pkce,
) -> Result<Tokens, String> {
    token_request(
        http,
        &discovery.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", &pkce.verifier),
            ("resource", &discovery.resource),
        ],
    )
    .await
}

/// A new access token from a refresh token, for the same resource.
pub async fn refresh(
    http: &reqwest::Client,
    token_endpoint: &str,
    client_id: &str,
    resource: &str,
    refresh_token: &str,
) -> Result<Tokens, String> {
    token_request(
        http,
        token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("resource", resource),
        ],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_https_origins() {
        assert_eq!(
            normalize_address("app.whity.dev").unwrap().as_str(),
            "https://app.whity.dev/"
        );
        assert_eq!(
            normalize_address(" https://app.whity.dev/home?x=1 ")
                .unwrap()
                .as_str(),
            "https://app.whity.dev/"
        );
        assert_eq!(
            normalize_address("http://localhost:8080").unwrap().as_str(),
            "http://localhost:8080/"
        );
        assert_eq!(
            normalize_address("http://whity.example.com").unwrap_err(),
            INVALID_ADDRESS
        );
        assert_eq!(
            normalize_address("ftp://app.whity.dev").unwrap_err(),
            INVALID_ADDRESS
        );
        assert_eq!(
            normalize_address("https://user:pw@app.whity.dev").unwrap_err(),
            INVALID_ADDRESS
        );
        assert_eq!(normalize_address("   ").unwrap_err(), INVALID_ADDRESS);
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        // RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let pkce = Pkce::new();
        assert_eq!(pkce.verifier.len(), 43);
        assert_ne!(Pkce::new().verifier, pkce.verifier);
    }

    fn discovery() -> Discovery {
        Discovery {
            resource: "https://app.whity.dev/mcp".into(),
            resource_name: Some("Whity".into()),
            issuer: "https://id.whity.dev".into(),
            authorization_endpoint: "https://id.whity.dev/authorize".into(),
            token_endpoint: "https://id.whity.dev/oauth/token".into(),
            registration_endpoint: Some("https://id.whity.dev/oauth/register".into()),
            scopes: vec!["tools:call".into(), "resources:read".into()],
        }
    }

    #[test]
    fn the_authorize_url_carries_pkce_state_and_resource() {
        let pkce = Pkce {
            verifier: "v".into(),
            challenge: "c".into(),
        };
        let url = authorize_url(
            &discovery(),
            "client-1",
            "http://127.0.0.1:5000/callback",
            &pkce,
            "s",
        )
        .unwrap();
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("code_challenge_method"), Some("S256"));
        assert_eq!(get("code_challenge"), Some("c"));
        assert_eq!(get("resource"), Some("https://app.whity.dev/mcp"));
        assert_eq!(get("scope"), Some("tools:call resources:read"));
        assert_eq!(get("redirect_uri"), Some("http://127.0.0.1:5000/callback"));
    }

    #[test]
    fn whity_id_uses_the_published_client_document() {
        assert_eq!(
            metadata_document_client_id("https://id.whity.dev/"),
            Some("https://id.whity.dev/clients/whity-agent.json")
        );
        assert_eq!(metadata_document_client_id("https://id.example.com"), None);
    }

    #[test]
    fn the_callback_is_checked() {
        let iss = "https://id.whity.dev";
        assert_eq!(
            parse_callback(
                "/callback?code=abc&state=s&iss=https%3A%2F%2Fid.whity.dev",
                "s",
                iss
            )
            .unwrap(),
            "abc"
        );
        assert_eq!(
            parse_callback("/callback?code=abc&state=s", "s", iss).unwrap(),
            "abc"
        );
        assert_eq!(
            parse_callback("/callback?code=abc&state=other", "s", iss).unwrap_err(),
            "state mismatch"
        );
        assert!(parse_callback(
            "/callback?code=abc&state=s&iss=https%3A%2F%2Fevil.example",
            "s",
            iss
        )
        .is_err());
        assert_eq!(
            parse_callback("/callback?error=access_denied&state=s", "s", iss).unwrap_err(),
            CANCELLED
        );
        assert!(parse_callback("/favicon.ico", "s", iss).is_err());
    }
}
