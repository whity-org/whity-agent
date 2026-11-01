//! Connecting a found tool to a signed-in workspace, and taking it out again
//! (WHIT-823).
//!
//! Always in two steps: [`Linker::plan`] says exactly what will happen (the
//! tool's own command, or the file's text before and after) and changes
//! nothing; [`Linker::connect`] does it only for the plan the person approved.
//! Every link is recorded, so whity-agent removes or restores only what it
//! added. No token is ever written: the tool gets the workspace's URL and
//! signs in itself.

use crate::config_writer::{self, Plan};
use crate::store::{Link, State};
use crate::tools::{self, Environment, Found, Method};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What connecting will do, for the person to approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Proposal {
    /// Run this command (argv, never through a shell).
    Command { argv: Vec<String>, server: String },
    /// Change this file.
    File { plan: Plan, server: String },
    /// Already connected exactly so; nothing to do.
    Nothing { server: String },
}

/// A found tool and the workspaces it is connected to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolView {
    #[serde(flatten)]
    pub found: Found,
    pub connections: Vec<String>,
}

/// The server name a workspace gets in a tool: `whity-` and a slug of its
/// name or host, which every tool accepts as a key and an argument.
pub fn server_name(workspace: &str) -> String {
    let slug: String = workspace
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "whity".into()
    } else {
        format!("whity-{slug}")
    }
}

pub struct Linker {
    pub env: Environment,
    pub backups: PathBuf,
    pub command_timeout: Duration,
    /// This executable, which a bridge tool's config runs as `mcp --connection <id>`.
    pub executable: Option<PathBuf>,
}

impl Linker {
    pub fn tools(&self, state: &State) -> Vec<ToolView> {
        self.env
            .detect()
            .into_iter()
            .map(|found| {
                let connections = state
                    .links
                    .iter()
                    .filter(|l| l.tool == found.id)
                    .map(|l| l.connection.clone())
                    .collect();
                ToolView { found, connections }
            })
            .collect()
    }

    /// What connecting `tool_id` to a workspace at `url` would do. Changes
    /// nothing. `bridge` is the connection id when its sign-in is in the keychain,
    /// which is what a bridge running later, on its own, needs.
    pub fn plan(
        &self,
        tool_id: &str,
        workspace: &str,
        url: &str,
        bridge: Option<&str>,
    ) -> Result<Proposal, String> {
        let tool = tools::tool(tool_id).ok_or("unknown tool")?;
        let server = server_name(workspace);
        match tool.method {
            Method::Bridge => {
                let connection = bridge.ok_or(
                    "this tool connects through Whity Agent, which keeps its sign-in in your computer's keychain, and that isn't available",
                )?;
                let shape = tool.json.ok_or("no config shape")?;
                let path = self
                    .env
                    .config_path(tool)
                    .ok_or("no config file for this computer")?;
                let executable = self
                    .executable
                    .as_ref()
                    .ok_or("can't find Whity Agent's own program")?;
                let args = vec![
                    "mcp".to_string(),
                    "--connection".to_string(),
                    connection.to_string(),
                ];
                let plan = config_writer::plan_add_command(
                    &path,
                    shape.container,
                    &server,
                    &executable.display().to_string(),
                    &args,
                )?;
                if plan.changes_nothing() {
                    Ok(Proposal::Nothing { server })
                } else {
                    Ok(Proposal::File { plan, server })
                }
            }
            Method::Command => {
                let add = tool.add.ok_or("no add command")?;
                let mut argv = add(&server, url);
                // Run the executable PATH would run, so a Windows `.cmd` shim works.
                if let Some(binary) = tool.binaries.first().and_then(|b| self.env.which(b)) {
                    argv[0] = binary.display().to_string();
                } else {
                    return Err(format!("{} isn't on PATH", tool.name));
                }
                Ok(Proposal::Command { argv, server })
            }
            Method::File => {
                let shape = tool.json.ok_or("no config shape")?;
                let path = self
                    .env
                    .config_path(tool)
                    .ok_or("no config file for this computer")?;
                let plan = config_writer::plan_add(&path, shape, &server, url)?;
                if plan.changes_nothing() {
                    Ok(Proposal::Nothing { server })
                } else {
                    Ok(Proposal::File { plan, server })
                }
            }
        }
    }

    /// Do what the person approved and return the record of it. The caller
    /// plans again here rather than taking a plan from the webview, so the
    /// webview can never choose a command or a path; a file is written only
    /// if it is still as it was when they reviewed it (`plan.before_hash`).
    pub async fn connect(
        &self,
        tool_id: &str,
        connection_id: &str,
        approved: &Proposal,
    ) -> Result<Link, String> {
        Ok(match approved {
            Proposal::Nothing { server } => Link {
                tool: tool_id.into(),
                connection: connection_id.into(),
                server: server.clone(),
                path: None,
                backup: None,
                written_hash: None,
            },
            Proposal::Command { argv, server } => {
                run(argv, self.command_timeout).await?;
                let tool = tools::tool(tool_id).ok_or("unknown tool")?;
                let path = self.env.config_path(tool).map(|p| p.display().to_string());
                Link {
                    tool: tool_id.into(),
                    connection: connection_id.into(),
                    server: server.clone(),
                    path,
                    backup: None,
                    written_hash: None,
                }
            }
            Proposal::File { plan, server } => {
                let applied = config_writer::apply(plan, &self.backups_for(tool_id))?;
                Link {
                    tool: tool_id.into(),
                    connection: connection_id.into(),
                    server: server.clone(),
                    path: Some(plan.path.clone()),
                    backup: applied.backup,
                    written_hash: Some(applied.written_hash),
                }
            }
        })
    }

    /// Take out what whity-agent added: the tool's own remove command, or our
    /// entry from its config file. Nothing else in the file changes.
    pub async fn disconnect(&self, link: &Link) -> Result<(), String> {
        let tool_id = link.tool.as_str();
        let tool = tools::tool(tool_id).ok_or("unknown tool")?;

        if let Some(remove) = tool.remove {
            let mut argv = remove(&link.server);
            if let Some(binary) = tool.binaries.first().and_then(|b| self.env.which(b)) {
                argv[0] = binary.display().to_string();
            }
            run(&argv, self.command_timeout).await?;
        } else if let (Some(shape), Some(path)) = (tool.json, self.env.config_path(tool)) {
            if let Some(plan) = config_writer::plan_remove(&path, shape.container, &link.server)? {
                config_writer::apply(&plan, &self.backups_for(tool_id))?;
            }
        }
        Ok(())
    }

    /// Put back the file as it was before whity-agent changed it.
    pub fn restore(&self, link: &Link) -> Result<(), String> {
        let (Some(path), Some(backup), Some(written)) =
            (&link.path, &link.backup, &link.written_hash)
        else {
            return Err("there is no backup for this connection".into());
        };
        config_writer::restore(
            Path::new(path),
            Path::new(backup),
            written,
            &self.backups_for(&link.tool),
        )
    }

    fn backups_for(&self, tool_id: &str) -> PathBuf {
        self.backups.join(tool_id)
    }
}

/// Run a tool's own command directly (no shell), with a time limit.
async fn run(argv: &[String], timeout: Duration) -> Result<(), String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::tools::no_console(&mut command);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| "the tool's command took too long".to_string())?
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "the tool's command failed: {}",
            stderr.lines().next().unwrap_or("no message")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("whity-agent-link-{}", crate::oauth::random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn linker(home: &Path, path: Vec<PathBuf>) -> Linker {
        Linker {
            env: Environment {
                os: "linux",
                home: Some(home.to_path_buf()),
                appdata: None,
                path,
            },
            backups: home.join("backups"),
            command_timeout: Duration::from_secs(10),
            executable: Some(home.join("whity-agent")),
        }
    }

    #[test]
    fn server_names_are_safe_slugs() {
        assert_eq!(server_name("Example Bakery"), "whity-example-bakery");
        assert_eq!(server_name("app.whity.dev"), "whity-app-whity-dev");
        assert_eq!(server_name("مخبز"), "whity");
    }

    #[tokio::test]
    async fn cursor_is_connected_by_a_reviewed_file_edit_and_restored() {
        let home = temp();
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        std::fs::write(
            home.join(".cursor/mcp.json"),
            r#"{"mcpServers":{"github":{"command":"gh"}}}"#,
        )
        .unwrap();
        let linker = linker(&home, vec![]);
        let mut state = State::default();

        let proposal = linker
            .plan(
                "cursor",
                "Example Bakery",
                "https://app.whity.dev/mcp",
                None,
            )
            .unwrap();
        let Proposal::File { plan, .. } = &proposal else {
            panic!("expected a file edit, got {proposal:?}")
        };
        assert!(plan.after.contains("\"whity-example-bakery\""));
        assert!(
            !std::fs::read_to_string(home.join(".cursor/mcp.json"))
                .unwrap()
                .contains("whity"),
            "planning writes nothing"
        );

        let link = linker.connect("cursor", "conn-1", &proposal).await.unwrap();
        assert!(std::fs::read_to_string(home.join(".cursor/mcp.json"))
            .unwrap()
            .contains("https://app.whity.dev/mcp"));
        state.links.push(link.clone());
        assert_eq!(
            linker
                .tools(&state)
                .iter()
                .find(|t| t.found.id == "cursor")
                .unwrap()
                .connections,
            vec!["conn-1"]
        );

        linker.restore(&link).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap(),
            r#"{"mcpServers":{"github":{"command":"gh"}}}"#
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn disconnecting_a_file_link_removes_only_our_entry() {
        let home = temp();
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        let linker = linker(&home, vec![]);
        let proposal = linker
            .plan(
                "cursor",
                "Example Bakery",
                "https://app.whity.dev/mcp",
                None,
            )
            .unwrap();
        let link = linker.connect("cursor", "conn-1", &proposal).await.unwrap();

        linker.disconnect(&link).await.unwrap();

        let text = std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap();
        assert!(!text.contains("whity-example-bakery"));
        linker.disconnect(&link).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap(),
            text,
            "a second disconnect changes nothing"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn a_tool_without_its_own_sign_in_runs_the_bridge_with_no_secret() {
        let home = temp();
        let linker = linker(&home, vec![]);

        let proposal = linker
            .plan(
                "windsurf",
                "app.whity.dev",
                "https://app.whity.dev/mcp",
                Some("conn-1"),
            )
            .unwrap();
        let Proposal::File { plan, server } = &proposal else {
            panic!("expected a file edit, got {proposal:?}")
        };
        assert_eq!(server, "whity-app-whity-dev");
        let after: serde_json::Value = serde_json::from_str(&plan.after).unwrap();
        let entry = &after["mcpServers"]["whity-app-whity-dev"];
        assert_eq!(
            entry["command"],
            home.join("whity-agent").display().to_string()
        );
        assert_eq!(
            entry["args"],
            serde_json::json!(["mcp", "--connection", "conn-1"])
        );
        assert!(
            !plan.after.to_lowercase().contains("bearer") && !plan.after.contains("https://"),
            "only a connection id"
        );

        let link = linker
            .connect("windsurf", "conn-1", &proposal)
            .await
            .unwrap();
        linker.disconnect(&link).await.unwrap();
        assert!(!std::fs::read_to_string(&plan.path)
            .unwrap()
            .contains("whity-app-whity-dev"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn without_a_keychain_a_bridge_tool_says_why() {
        let home = temp();
        let error = linker(&home, vec![])
            .plan(
                "windsurf",
                "app.whity.dev",
                "https://app.whity.dev/mcp",
                None,
            )
            .unwrap_err();
        assert!(error.contains("keychain"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_command_runs_the_binary_found_on_path_with_the_url_only() {
        let home = temp();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("claude"), b"").unwrap();

        let proposal = linker(&home, vec![bin.clone()])
            .plan(
                "claude-code",
                "Example Bakery",
                "https://app.whity.dev/mcp",
                None,
            )
            .unwrap();
        let Proposal::Command { argv, server } = proposal else {
            panic!()
        };
        assert_eq!(argv[0], bin.join("claude").display().to_string());
        assert_eq!(
            &argv[1..],
            [
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                "whity-example-bakery",
                "https://app.whity.dev/mcp"
            ]
        );
        assert_eq!(server, "whity-example-bakery");
        assert!(
            linker(&home, vec![])
                .plan("claude-code", "x", "https://app.whity.dev/mcp", None)
                .is_err(),
            "not installed"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_is_reported_and_not_recorded() {
        let home = temp();
        let failing = Proposal::Command {
            argv: vec!["false".into()],
            server: "whity-x".into(),
        };

        assert!(linker(&home, vec![])
            .connect("claude-code", "conn-1", &failing)
            .await
            .is_err());
        std::fs::remove_dir_all(home).unwrap();
    }
}
