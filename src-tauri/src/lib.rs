//! whity-agent's native side. The webview only calls the commands below; every
//! network request, token, keychain entry, file write and tool command stays
//! in Rust. No command takes a file path or a command line from the webview.

mod bridge;
mod chat;
mod config_writer;
mod connections;
mod linking;
mod loopback;
mod oauth;
mod store;
mod tools;

use chat::{BridgeCommand, ChatEvent, Engine};
use connections::{ConnectionView, Connections};
use linking::{Linker, Proposal, ToolView};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use store::{OsKeychain, StateFile};
use tauri::ipc::Channel;
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
    let (host, url, persisted) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    linker.plan(&tool, &host, &url, persisted.then_some(connection.as_str()))
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
    let (host, url, persisted) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    let proposal = linker.plan(&tool, &host, &url, persisted.then_some(connection.as_str()))?;
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

/// The chat's running turn and conversations. Engine session ids stay here: the
/// webview never sees them or chooses which to resume.
struct ChatState {
    dir: PathBuf,
    running: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    sessions: Mutex<HashMap<(String, String), String>>,
}

#[derive(serde::Serialize)]
struct EngineView {
    id: &'static str,
    name: &'static str,
}

/// The tools on this computer that can answer a chat.
#[tauri::command]
fn list_engines(linker: tauri::State<'_, Linker>) -> Vec<EngineView> {
    Engine::ALL
        .into_iter()
        .filter(|e| e.locate(&linker.env).is_some())
        .map(|e| EngineView {
            id: e.id(),
            name: e.name(),
        })
        .collect()
}

/// One turn of the chat, its answer arriving on `on_event`. One at a time.
#[tauri::command]
async fn send_chat(
    connections: tauri::State<'_, Connections>,
    linker: tauri::State<'_, Linker>,
    chat: tauri::State<'_, ChatState>,
    engine: String,
    connection: String,
    message: String,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    let kind = Engine::from_id(&engine).ok_or("unknown tool")?;
    let program = kind
        .locate(&linker.env)
        .ok_or("that tool isn't on this computer")?;
    let (_, _, persisted) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    if !persisted {
        return Err("this workspace's sign-in isn't in your computer's keychain".into());
    }
    let executable = linker
        .executable
        .as_ref()
        .ok_or("can't find Whity Agent's own program")?;
    let bridge = BridgeCommand {
        program: executable.display().to_string(),
        connection: connection.clone(),
    };

    let (stop, stopped) = tokio::sync::oneshot::channel();
    {
        let mut running = chat.running.lock().unwrap();
        if running.is_some() {
            return Err("a reply is still coming".into());
        }
        *running = Some(stop);
    }
    let key = (engine, connection);
    let session = chat.sessions.lock().unwrap().get(&key).cloned();
    let result = async {
        let turn = chat::prepare_turn(kind, &bridge, &chat.dir, session.as_deref(), &message)?;
        chat::run_program(
            &program,
            &turn.args,
            &chat.dir,
            turn.input,
            chat::parser(kind),
            |event| match event {
                ChatEvent::Session { id } => {
                    chat.sessions.lock().unwrap().insert(key.clone(), id);
                }
                event => {
                    let _ = on_event.send(event);
                }
            },
            stopped,
        )
        .await
    }
    .await;
    chat.running.lock().unwrap().take();
    result
}

#[tauri::command]
fn stop_chat(chat: tauri::State<'_, ChatState>) {
    if let Some(stop) = chat.running.lock().unwrap().take() {
        let _ = stop.send(());
    }
}

/// Forget the conversations, so the next message starts a new one.
#[tauri::command]
fn new_chat(chat: tauri::State<'_, ChatState>) {
    chat.sessions.lock().unwrap().clear();
}

/// The workspace's approvals, in the browser: the desktop isn't a second inbox.
#[tauri::command]
fn open_approvals(
    app: tauri::AppHandle,
    connections: tauri::State<'_, Connections>,
    connection: String,
) -> Result<(), String> {
    let (_, resource, _) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    let url = url::Url::parse(&resource)
        .and_then(|u| u.join("/admin/approvals"))
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// `whity-agent mcp --connection <id>`: the stdio bridge, with no window. It finds
/// its state where the app keeps it: Tauri's app config dir is the OS config
/// folder and the identifier in tauri.conf.json. Returns the process's exit code.
pub fn run_bridge(args: &[String]) -> i32 {
    let connection = match args {
        [flag, id] if flag == "--connection" => id,
        _ => {
            eprintln!("usage: whity-agent mcp --connection <id>");
            return 2;
        }
    };
    let Some(dir) = dirs::config_dir().map(|d| d.join("dev.whity.agent")) else {
        eprintln!("whity-agent: can't find the config folder");
        return 1;
    };
    let connections = Connections::new(StateFile::new(&dir), Box::new(OsKeychain));
    let Some((_, url, true)) = connections.workspace(connection) else {
        eprintln!("whity-agent: not signed in to that workspace; open Whity Agent and sign in");
        return 1;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("whity-agent: {e}");
            return 1;
        }
    };
    match runtime.block_on(bridge::Bridge::new(&connections, connection, &url).run()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("whity-agent: {e}");
            1
        }
    }
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
                executable: std::env::current_exe().ok(),
            });
            app.manage(ChatState {
                dir: app.path().app_data_dir()?.join("chat"),
                running: Mutex::new(None),
                sessions: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_connections,
            list_engines,
            send_chat,
            stop_chat,
            new_chat,
            open_approvals,
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
