//! whity-agent's native side. The webview only calls the commands below; every
//! network request, token and keychain entry stays in Rust.

mod connections;
mod loopback;
mod oauth;
mod store;

use connections::{ConnectionView, Connections};
use store::{OsKeychain, StateFile};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
fn list_connections(connections: tauri::State<'_, Connections>) -> Vec<ConnectionView> {
    connections.list()
}

#[tauri::command]
async fn sign_in(
    app: tauri::AppHandle,
    connections: tauri::State<'_, Connections>,
    address: String,
) -> Result<ConnectionView, String> {
    connections
        .sign_in(&address, |url| {
            app.opener()
                .open_url(url.as_str(), None::<&str>)
                .map_err(|e| e.to_string())
        })
        .await
}

#[tauri::command]
fn sign_out(connections: tauri::State<'_, Connections>, id: String) -> Result<(), String> {
    connections.sign_out(&id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            app.manage(Connections::new(StateFile::new(&dir), Box::new(OsKeychain)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_connections,
            sign_in,
            sign_out
        ])
        .run(tauri::generate_context!())
        .expect("error while running whity-agent");
}
