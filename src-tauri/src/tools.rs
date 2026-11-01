//! The AI tools whity-agent can connect, and finding them on this computer
//! (WHIT-823).
//!
//! The table mirrors `local` in whity-core's `@whity/features/mcp`
//! (`connect-clients.ts`), so the web connect wizard and this app describe
//! each tool the same way. Keep the two in step.
//!
//! Detection only LOOKS: it searches PATH for the tool's executables and
//! checks whether its config file exists. It never runs anything.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// How whity-agent connects a tool, without ever writing a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    /// Run the tool's own "add server" command with the URL; the tool signs in itself.
    Command,
    /// Add a URL-only entry to the tool's JSON config; the tool signs in itself.
    File,
    /// The tool can't sign in itself: its JSON config runs whity-agent's local
    /// bridge as a command, which holds the sign-in (WHIT-824).
    Bridge,
}

/// Where a URL-only entry goes in a JSON config: `{ <container>: { <name>: { <url_key>: url } } }`.
#[derive(Debug, Clone, Copy)]
pub struct JsonEntry {
    pub container: &'static str,
    pub url_key: &'static str,
    /// An extra `"type": "http"` some tools want beside the URL.
    pub http_type: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    /// (os, paths) with `~` and `%APPDATA%` placeholders.
    pub config_paths: &'static [(&'static str, &'static [&'static str])],
    pub method: Method,
    /// For `Method::Command`: argv to add, and to remove, a server.
    pub add: Option<fn(&str, &str) -> Vec<String>>,
    pub remove: Option<fn(&str) -> Vec<String>>,
    /// The JSON shape of its user config, for `Method::File` and for removal.
    pub json: Option<JsonEntry>,
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_string()).collect()
}

pub const TOOLS: &[Tool] = &[
    Tool {
        id: "claude-code",
        name: "Claude Code",
        binaries: &["claude"],
        config_paths: &[
            ("macos", &["~/.claude.json"]),
            ("windows", &["~/.claude.json"]),
            ("linux", &["~/.claude.json"]),
        ],
        method: Method::Command,
        add: Some(|name, url| {
            argv(&[
                "claude",
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "http",
                name,
                url,
            ])
        }),
        remove: Some(|name| argv(&["claude", "mcp", "remove", "--scope", "user", name])),
        json: None,
    },
    Tool {
        id: "codex",
        name: "Codex CLI",
        binaries: &["codex"],
        config_paths: &[
            ("macos", &["~/.codex/config.toml"]),
            ("windows", &["~/.codex/config.toml"]),
            ("linux", &["~/.codex/config.toml"]),
        ],
        method: Method::Command,
        add: Some(|name, url| argv(&["codex", "mcp", "add", name, "--url", url])),
        remove: Some(|name| argv(&["codex", "mcp", "remove", name])),
        json: None,
    },
    Tool {
        id: "gemini-cli",
        name: "Gemini CLI",
        binaries: &["gemini"],
        config_paths: &[
            ("macos", &["~/.gemini/settings.json"]),
            ("windows", &["~/.gemini/settings.json"]),
            ("linux", &["~/.gemini/settings.json"]),
        ],
        method: Method::Command,
        add: Some(|name, url| {
            argv(&[
                "gemini",
                "mcp",
                "add",
                "--transport",
                "http",
                "--scope",
                "user",
                name,
                url,
            ])
        }),
        remove: Some(|name| argv(&["gemini", "mcp", "remove", "--scope", "user", name])),
        json: None,
    },
    Tool {
        id: "vscode",
        name: "VS Code",
        binaries: &["code"],
        config_paths: &[
            (
                "macos",
                &["~/Library/Application Support/Code/User/mcp.json"],
            ),
            ("windows", &["%APPDATA%\\Code\\User\\mcp.json"]),
            ("linux", &["~/.config/Code/User/mcp.json"]),
        ],
        method: Method::Command,
        add: Some(|name, url| {
            let entry = serde_json::json!({ "name": name, "type": "http", "url": url });
            vec!["code".into(), "--add-mcp".into(), entry.to_string()]
        }),
        // No remove command: the entry is taken out of its mcp.json.
        remove: None,
        json: Some(JsonEntry {
            container: "servers",
            url_key: "url",
            http_type: true,
        }),
    },
    Tool {
        id: "cursor",
        name: "Cursor",
        binaries: &["cursor"],
        config_paths: &[
            ("macos", &["~/.cursor/mcp.json"]),
            ("windows", &["~/.cursor/mcp.json"]),
            ("linux", &["~/.cursor/mcp.json"]),
        ],
        method: Method::File,
        add: None,
        remove: None,
        json: Some(JsonEntry {
            container: "mcpServers",
            url_key: "url",
            http_type: false,
        }),
    },
    Tool {
        id: "windsurf",
        name: "Windsurf",
        binaries: &["windsurf"],
        config_paths: &[
            ("macos", &["~/.config/devin/mcp_config.json"]),
            ("windows", &["%APPDATA%\\devin\\mcp_config.json"]),
            ("linux", &["~/.config/devin/mcp_config.json"]),
        ],
        method: Method::Bridge,
        add: None,
        remove: None,
        json: Some(JsonEntry {
            container: "mcpServers",
            url_key: "url",
            http_type: false,
        }),
    },
    Tool {
        id: "claude",
        name: "Claude Desktop",
        binaries: &[],
        config_paths: &[
            (
                "macos",
                &["~/Library/Application Support/Claude/claude_desktop_config.json"],
            ),
            (
                "windows",
                &["%APPDATA%\\Claude\\claude_desktop_config.json"],
            ),
        ],
        method: Method::Bridge,
        add: None,
        remove: None,
        json: Some(JsonEntry {
            container: "mcpServers",
            url_key: "url",
            http_type: false,
        }),
    },
];

pub fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

/// What detection found for one tool.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub id: &'static str,
    pub name: &'static str,
    pub method: Method,
    pub binary: Option<String>,
    pub config: Option<String>,
    pub installed: bool,
}

/// Where a user's things are, injected so tests use a temporary folder.
pub struct Environment {
    pub os: &'static str,
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub path: Vec<PathBuf>,
}

impl Environment {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS,
            home: std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            path: std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).collect())
                .unwrap_or_default(),
        }
    }

    /// `~/x` and `%APPDATA%\x` to real paths; None when the base is unknown.
    pub fn expand(&self, pattern: &str) -> Option<PathBuf> {
        if let Some(rest) = pattern.strip_prefix("~/") {
            return self.home.as_ref().map(|h| h.join(rest));
        }
        if let Some(rest) = pattern.strip_prefix("%APPDATA%\\") {
            return self
                .appdata
                .as_ref()
                .map(|a| a.join(rest.replace('\\', "/")));
        }
        Some(PathBuf::from(pattern))
    }

    /// The first config path of a tool for this OS.
    pub fn config_path(&self, tool: &Tool) -> Option<PathBuf> {
        let os = self.os;
        tool.config_paths
            .iter()
            .find(|(o, _)| *o == os)
            .and_then(|(_, paths)| paths.first())
            .and_then(|p| self.expand(p))
    }

    /// An executable on PATH, as the OS would find it (with Windows' extensions).
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> = if self.os == "windows" {
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| format!("{name}.{ext}"))
                .collect()
        } else {
            vec![name.to_string()]
        };
        self.path
            .iter()
            .find_map(|dir| names.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
    }

    pub fn detect(&self) -> Vec<Found> {
        TOOLS
            .iter()
            .filter(|t| t.config_paths.iter().any(|(o, _)| *o == self.os) || !t.binaries.is_empty())
            .map(|t| {
                let binary = t.binaries.iter().find_map(|b| self.which(b));
                let config = self
                    .config_path(t)
                    .filter(|p| p.is_file() || p.parent().is_some_and(Path::is_dir));
                Found {
                    id: t.id,
                    name: t.name,
                    method: t.method,
                    installed: binary.is_some() || config.is_some(),
                    binary: binary.map(|p| p.display().to_string()),
                    config: config.map(|p| p.display().to_string()),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "whity-agent-tools-{}",
            crate::oauth::random_token()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn it_finds_tools_by_path_and_by_config_without_running_them() {
        let home = temp();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("claude"), b"").unwrap();
        std::fs::create_dir_all(home.join(".cursor")).unwrap();
        let env = Environment {
            os: "linux",
            home: Some(home.clone()),
            appdata: None,
            path: vec![bin],
        };

        let found = env.detect();
        let get = |id: &str| found.iter().find(|f| f.id == id).cloned().unwrap();

        assert!(get("claude-code").installed && get("claude-code").binary.is_some());
        assert!(get("cursor").installed, "its config folder exists");
        assert!(!get("codex").installed);
        assert!(
            found.iter().all(|f| f.id != "claude"),
            "Claude Desktop isn't on Linux"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn windows_finds_cmd_shims_and_expands_appdata() {
        let home = temp();
        std::fs::write(home.join("code.cmd"), b"").unwrap();
        let env = Environment {
            os: "windows",
            home: Some(home.clone()),
            appdata: Some(home.join("AppData")),
            path: vec![home.clone()],
        };

        assert_eq!(env.which("code"), Some(home.join("code.cmd")));
        assert_eq!(
            env.config_path(tool("vscode").unwrap()),
            Some(home.join("AppData").join("Code/User/mcp.json"))
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn no_command_or_entry_carries_a_token() {
        for t in TOOLS {
            if let Some(add) = t.add {
                let argv = add("whity-example-bakery", "https://app.whity.dev/mcp").join(" ");
                assert!(
                    !argv.to_lowercase().contains("bearer")
                        && !argv.to_lowercase().contains("authorization"),
                    "{}",
                    t.id
                );
                assert!(argv.contains("https://app.whity.dev/mcp"));
            }
            assert!(
                t.method != Method::Command || t.add.is_some(),
                "{} needs its add command",
                t.id
            );
            assert!(
                t.method != Method::File || t.json.is_some(),
                "{} needs its JSON shape",
                t.id
            );
        }
    }
}
