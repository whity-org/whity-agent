//! What whity-agent keeps between runs.
//!
//! - **Secrets** (refresh tokens) go to the OS keychain only, through
//!   [`SecretStore`]: Windows Credential Manager, macOS Keychain, Linux Secret
//!   Service. If the keychain is unavailable, nothing secret is written
//!   anywhere and the sign-in lasts until the app closes. There is no file
//!   fallback, by design.
//! - **Everything else** (which workspaces, which authorization server, the
//!   public client id registered there) goes to `state.json` in the app's
//!   config folder. None of it is secret: a public client id is not a
//!   credential.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const KEYCHAIN_SERVICE: &str = "dev.whity.agent";

/// One signed-in workspace. Serialised to `state.json`: no secrets here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub resource: String,
    pub name: Option<String>,
    pub issuer: String,
    pub token_endpoint: String,
    pub client_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub connections: Vec<Connection>,
    /// The public client id whity-agent registered at each authorization
    /// server, by issuer, so it registers once rather than on every sign-in.
    #[serde(default)]
    pub clients: BTreeMap<String, String>,
}

/// A stable id per workspace resource, so signing in again replaces the old
/// connection instead of adding a second one.
pub fn connection_id(resource: &str) -> String {
    let digest = Sha256::digest(resource.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn keychain_account(connection_id: &str) -> String {
    format!("connection:{connection_id}")
}

pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("state.json"),
        }
    }

    /// The saved state, or an empty one when there is none yet. An unreadable
    /// file is treated as empty rather than blocking the app: it holds nothing
    /// that can't be recreated by signing in again.
    pub fn load(&self) -> State {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write via a temporary file and a rename, so a crash never leaves half a file.
    pub fn save(&self, state: &State) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

/// Where refresh tokens live. The real one is the OS keychain; tests use memory.
pub trait SecretStore: Send + Sync {
    fn set(&self, account: &str, secret: &str) -> Result<(), String>;
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

pub struct OsKeychain;

impl OsKeychain {
    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| format!("keychain unavailable: {e}"))
    }
}

impl SecretStore for OsKeychain {
    fn set(&self, account: &str, secret: &str) -> Result<(), String> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|e| format!("keychain write failed: {e}"))
    }

    fn get(&self, account: &str) -> Result<Option<String>, String> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("keychain read failed: {e}")),
        }
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("keychain delete failed: {e}")),
        }
    }
}

#[cfg(test)]
pub mod memory {
    use super::SecretStore;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A keychain stand-in for tests; `broken` makes every call fail, like a
    /// Linux desktop without a Secret Service.
    #[derive(Default)]
    pub struct MemoryStore {
        pub secrets: Mutex<HashMap<String, String>>,
        pub broken: bool,
    }

    impl SecretStore for MemoryStore {
        fn set(&self, account: &str, secret: &str) -> Result<(), String> {
            if self.broken {
                return Err("keychain unavailable".into());
            }
            self.secrets
                .lock()
                .unwrap()
                .insert(account.into(), secret.into());
            Ok(())
        }
        fn get(&self, account: &str) -> Result<Option<String>, String> {
            if self.broken {
                return Err("keychain unavailable".into());
            }
            Ok(self.secrets.lock().unwrap().get(account).cloned())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.secrets.lock().unwrap().remove(account);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("whity-agent-test-{}", crate::oauth::random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn state_round_trips_and_holds_no_token_fields() {
        let dir = temp_dir();
        let file = StateFile::new(&dir);
        assert!(file.load().connections.is_empty());

        let mut state = State::default();
        state.connections.push(Connection {
            id: connection_id("https://app.whity.dev/mcp"),
            resource: "https://app.whity.dev/mcp".into(),
            name: Some("Whity".into()),
            issuer: "https://id.whity.dev".into(),
            token_endpoint: "https://id.whity.dev/oauth/token".into(),
            client_id: "client-1".into(),
        });
        state
            .clients
            .insert("https://id.whity.dev".into(), "client-1".into());
        file.save(&state).unwrap();

        let raw = std::fs::read_to_string(dir.join("state.json")).unwrap();
        assert!(!raw.contains("access_token") && !raw.contains("refresh_token"));
        assert_eq!(file.load().connections, state.connections);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ids_are_stable_per_resource() {
        assert_eq!(
            connection_id("https://app.whity.dev/mcp"),
            connection_id("https://app.whity.dev/mcp")
        );
        assert_ne!(
            connection_id("https://app.whity.dev/mcp"),
            connection_id("https://other.whity.dev/mcp")
        );
        assert_eq!(connection_id("x").len(), 16);
    }
}
