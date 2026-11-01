//! whity-agent's native side. The webview only calls the commands below; every
//! network request, token, keychain entry, file write and tool command stays
//! in Rust. No command takes a file path or a command line from the webview.

mod config_writer;
mod connections;
mod linking;
mod loopback;
mod oauth;
mod store;
mod tools;

use connections::{ConnectionView, Connections};
use linking::{Linker, Proposal, ToolView};
use std::time::Duration;
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
    connections.update_state(|state| state.links.retain(|l| l.connection != id))?;
    connections.sign_out(&id)
}

/// The AI tools found on this computer, and the workspaces each is connected to.
#[tauri::command]
fn list_tools(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
) -> Vec<ToolView> {
    linker.tools(&connections.state())
}

/// What connecting a tool to a workspace would do, for the person to review. Changes nothing.
#[tauri::command]
fn plan_connect(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
    tool: String,
    connection: String,
) -> Result<Proposal, String> {
    let (host, url) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    linker.plan(&tool, &host, &url)
}

/// Do it. The plan is made again here; for a file edit, `approved_hash` is the
/// hash of the text the person reviewed, and nothing is written if the file
/// has changed since.
#[tauri::command]
async fn connect_tool(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
    tool: String,
    connection: String,
    approved_hash: Option<String>,
) -> Result<(), String> {
    let (host, url) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    let proposal = linker.plan(&tool, &host, &url)?;
    if let Proposal::File { plan, .. } = &proposal {
        if approved_hash.as_deref() != Some(plan.before_hash.as_str()) {
            return Err(
                "the file changed since you reviewed it; nothing was written. Review it again."
                    .into(),
            );
        }
    }
    let link = linker.connect(&tool, &connection, &proposal).await?;
    connections.update_state(|state| {
        state
            .links
            .retain(|l| !(l.tool == tool && l.connection == connection));
        state.links.push(link);
    })
}

#[tauri::command]
async fn disconnect_tool(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
    tool: String,
    connection: String,
) -> Result<(), String> {
    let link = connections
        .state()
        .links
        .into_iter()
        .find(|l| l.tool == tool && l.connection == connection)
        .ok_or("not connected by whity-agent")?;
    linker.disconnect(&link).await?;
    connections.update_state(|state| {
        state
            .links
            .retain(|l| !(l.tool == tool && l.connection == connection))
    })
}

#[tauri::command]
fn restore_tool(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
    tool: String,
    connection: String,
) -> Result<(), String> {
    let link = connections
        .state()
        .links
        .into_iter()
        .find(|l| l.tool == tool && l.connection == connection)
        .ok_or("not connected by whity-agent")?;
    linker.restore(&link)?;
    connections.update_state(|state| {
        state
            .links
            .retain(|l| !(l.tool == tool && l.connection == connection))
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            app.manage(Connections::new(StateFile::new(&dir), Box::new(OsKeychain)));
            app.manage(Linker {
                env: tools::Environment::current(),
                backups: app.path().app_data_dir()?.join("backups"),
                command_timeout: Duration::from_secs(60),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_connections,
            sign_in,
            sign_out,
            list_tools,
            plan_connect,
            connect_tool,
            disconnect_tool,
            restore_tool
        ])
        .run(tauri::generate_context!())
        .expect("error while running whity-agent");
}
