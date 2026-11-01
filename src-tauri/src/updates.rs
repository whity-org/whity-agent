//! Signed updates (WHIT-824, design §5.7): a static `latest.json` on GitHub
//! Releases, checked against the minisign public key built into the app.
//! OS code signing is off for now, by decision (Q5), but updates are always
//! signed: an update that doesn't verify is never installed.

use std::sync::Mutex;
use tauri_plugin_updater::{Update, UpdaterExt};

/// Empty until the release key exists; the updater is off while it is.
const PUBLIC_KEY: &str = include_str!("../updater.pub");
const ENDPOINT: &str =
    "https://github.com/whity-org/whity-agent/releases/latest/download/latest.json";

/// An update already downloaded, waiting for the person to restart.
#[derive(Default)]
pub struct PendingUpdate(pub Mutex<Option<(Update, Vec<u8>)>>);

fn public_key() -> Option<&'static str> {
    Some(PUBLIC_KEY.trim()).filter(|key| !key.is_empty())
}

/// Look for a newer version and download it. Returns its version when one is ready.
pub async fn check(
    app: &tauri::AppHandle,
    pending: &PendingUpdate,
) -> Result<Option<String>, String> {
    let Some(key) = public_key().filter(|_| !cfg!(debug_assertions)) else {
        return Ok(None);
    };
    let endpoint = url::Url::parse(ENDPOINT).map_err(|e| e.to_string())?;
    let update = app
        .updater_builder()
        .pubkey(key)
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    let version = update.version.clone();
    if let Some((ready, _)) = pending.0.lock().unwrap().as_ref() {
        if ready.version == version {
            return Ok(Some(version));
        }
    }
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    *pending.0.lock().unwrap() = Some((update, bytes));
    Ok(Some(version))
}

/// Install what was downloaded and restart. On Windows the installer ends this
/// process itself.
pub fn install(app: &tauri::AppHandle, pending: &PendingUpdate) -> Result<(), String> {
    let (update, bytes) = pending
        .0
        .lock()
        .unwrap()
        .take()
        .ok_or("no update is waiting")?;
    update.install(bytes).map_err(|e| e.to_string())?;
    app.restart()
}
