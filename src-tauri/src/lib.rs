//! whity-agent's native side. The webview only calls the commands below; every
//! network request, token, keychain entry, file write and tool command stays
//! in Rust. No command takes a file path or a command line from the webview.

mod bridge;
mod chat;
mod config_writer;
mod connections;
mod desktop;
mod linking;
mod loopback;
mod oauth;
mod store;
mod tools;
mod updates;

use chat::{BridgeCommand, ChatEvent, Engine};
use connections::{ConnectionView, Connections};
use linking::{Linker, Proposal, ToolView};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use store::{OsKeychain, StateFile};
use tauri::ipc::Channel;
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_deep_link::DeepLinkExt as _;
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

/// The agents the person granted in a workspace. Nothing for an agent's own sign-in.
#[tauri::command]
async fn list_granted_agents(
    connections: tauri::State<'_, Connections>,
    connection: String,
    language: String,
) -> Result<Vec<bridge::GrantedAgent>, String> {
    let (_, resource, _) = connections
        .workspace(&connection)
        .ok_or("not signed in to that workspace")?;
    let language = if language == "ar" { "ar" } else { "en" };
    bridge::granted_agents(&connections, &connection, &resource, language).await
}

/// Sign in as one of the workspace's agents, in the browser.
#[tauri::command]
async fn sign_in_agent(
    app: tauri::AppHandle,
    connections: tauri::State<'_, Connections>,
    connection: String,
    slug: String,
) -> Result<ConnectionView, String> {
    connections
        .sign_in_agent(&connection, &slug, |url| {
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

/// The path a tool's config can run this app by. An AppImage runs from a temporary
/// mount that's gone next time, and says where the real file is in `APPIMAGE`.
fn own_executable() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// See [`tools::with_usual_dirs`]. Windows apps get the full PATH already.
fn add_usual_dirs_to_path() {
    if cfg!(windows) {
        return;
    }
    let path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Ok(joined) = std::env::join_paths(tools::with_usual_dirs(path, home.as_deref())) {
        // During setup, before the app starts any thread of its own that reads it.
        std::env::set_var("PATH", joined);
    }
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

/// Look for a newer version and download it, returning its version when it's ready.
#[tauri::command]
async fn check_for_update(
    app: tauri::AppHandle,
    pending: tauri::State<'_, updates::PendingUpdate>,
) -> Result<Option<String>, String> {
    updates::check(&app, &pending).await
}

#[tauri::command]
fn install_update(
    app: tauri::AppHandle,
    pending: tauri::State<'_, updates::PendingUpdate>,
) -> Result<(), String> {
    updates::install(&app, &pending)
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

/// A workspace address a `whity-agent://` link brought, once.
#[tauri::command]
fn take_connect_link(pending: tauri::State<'_, desktop::PendingLink>) -> Option<String> {
    pending.0.lock().unwrap().take()
}

/// The tray's words, in the language the person chose.
#[tauri::command]
fn set_tray_labels(app: tauri::AppHandle, open: String, quit: String) -> Result<(), String> {
    desktop::set_tray_labels(&app, &open, &quit)
}

#[tauri::command]
fn get_start_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_start_at_login(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let autostart = app.autolaunch();
    if enabled {
        autostart.enable()
    } else {
        autostart.disable()
    }
    .map_err(|e| e.to_string())
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
        // First, as the plugin asks: a second start only hands over its links.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            desktop::show_window(app);
            desktop::handle_links(app, args.iter().map(String::as_str));
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![desktop::HIDDEN_ARG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::PendingUpdate::default())
        .manage(desktop::PendingLink::default())
        // Closing the window keeps the app in the tray; Quit there ends it.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            add_usual_dirs_to_path();
            desktop::create_tray(app)?;
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                let links: Vec<String> = event.urls().iter().map(|u| u.to_string()).collect();
                desktop::handle_links(&handle, links.iter().map(String::as_str));
            });
            // Windows and Linux deliver a link to a fresh start as an argument.
            // The installers register the scheme, except for an AppImage.
            #[cfg(target_os = "linux")]
            let _ = app.deep_link().register_all();
            let args: Vec<String> = std::env::args().skip(1).collect();
            desktop::handle_links(app.handle(), args.iter().map(String::as_str));
            if !args.iter().any(|a| a == desktop::HIDDEN_ARG) {
                desktop::show_window(app.handle());
            }
            let dir = app.path().app_config_dir()?;
            app.manage(Connections::new(StateFile::new(&dir), Box::new(OsKeychain)));
            app.manage(Linker {
                env: tools::Environment::current(),
                backups: app.path().app_data_dir()?.join("backups"),
                command_timeout: Duration::from_secs(60),
                executable: own_executable(),
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
            list_granted_agents,
            sign_in_agent,
            list_engines,
            take_connect_link,
            set_tray_labels,
            get_start_at_login,
            set_start_at_login,
            check_for_update,
            install_update,
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
