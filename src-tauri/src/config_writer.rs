//! Changing another app's JSON config, safely (WHIT-823).
//!
//! 1. **Plan first.** [`plan_add`] and [`plan_remove`] read the file and
//!    return the text before and after, for the person to see. Nothing is
//!    written until they say yes.
//! 2. **Only our entry.** The plan touches one key under the app's server
//!    list. An entry of the same name that differs is a conflict, never
//!    overwritten. A file that isn't plain JSON (comments, trailing commas)
//!    is left alone: the person is told to use the app's own setting.
//! 3. **Nothing changed meanwhile.** [`apply`] re-reads the file and refuses
//!    if it differs from what was planned.
//! 4. **Backed up, written atomically.** The current file is copied to the
//!    app's backups folder (the newest 20 kept), the new text is written to a
//!    temporary file beside it, then renamed over it.
//! 5. **A way back.** [`restore`] puts the backup back, if the file is still
//!    what whity-agent wrote.
//!
//! Never a token: an entry holds the server's URL only.

use crate::tools::JsonEntry;
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEEP_BACKUPS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub path: String,
    pub before: String,
    pub after: String,
    /// Hash of `before` (empty string: the file didn't exist).
    pub before_hash: String,
}

impl Plan {
    pub fn changes_nothing(&self) -> bool {
        self.before == self.after
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub backup: Option<String>,
    pub written_hash: String,
}

pub fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {}: {e}", path.display())),
    }
}

fn parse(path: &Path, text: &str) -> Result<Map<String, Value>, String> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(format!(
            "{} isn't plain JSON (it may have comments), so it was left alone; add the server in the app's own settings",
            path.display()
        )),
    }
}

fn render(map: &Map<String, Value>) -> String {
    let mut text = serde_json::to_string_pretty(map).unwrap_or_default();
    text.push('\n');
    text
}

/// The text with a URL-only entry `name` added under the app's server list.
pub fn plan_add(path: &Path, shape: JsonEntry, name: &str, url: &str) -> Result<Plan, String> {
    let before = read(path)?.unwrap_or_default();
    let mut entry = Map::new();
    if shape.http_type {
        entry.insert("type".into(), Value::String("http".into()));
    }
    entry.insert(shape.url_key.into(), Value::String(url.into()));
    plan_entry(path, shape.container, name, Value::Object(entry), before)
}

/// The text with a command entry `name` added: whity-agent's bridge, which holds
/// no secret, only a connection id in its arguments.
pub fn plan_add_command(
    path: &Path,
    container: &str,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<Plan, String> {
    let before = read(path)?.unwrap_or_default();
    plan_entry(
        path,
        container,
        name,
        serde_json::json!({ "command": command, "args": args }),
        before,
    )
}

fn plan_entry(
    path: &Path,
    container: &str,
    name: &str,
    entry: Value,
    before: String,
) -> Result<Plan, String> {
    let mut root = parse(path, &before)?;

    let servers = root
        .entry(container)
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(servers) = servers else {
        return Err(format!(
            "{} has an unexpected `{}`; left alone",
            path.display(),
            container
        ));
    };
    match servers.get(name) {
        Some(existing) if *existing == entry => {
            return Ok(Plan {
                path: path.display().to_string(),
                after: before.clone(),
                before_hash: hash(&before),
                before,
            });
        }
        Some(_) => {
            return Err(format!(
                "{} already has a different server named {name}; it was left alone",
                path.display()
            ))
        }
        None => {
            servers.insert(name.into(), entry);
        }
    }

    Ok(Plan {
        path: path.display().to_string(),
        after: render(&root),
        before_hash: hash(&before),
        before,
    })
}

/// The text without entry `name`; None when there's nothing to remove.
pub fn plan_remove(path: &Path, container: &str, name: &str) -> Result<Option<Plan>, String> {
    let Some(before) = read(path)? else {
        return Ok(None);
    };
    let mut root = parse(path, &before)?;
    let removed = root
        .get_mut(container)
        .and_then(Value::as_object_mut)
        .and_then(|servers| servers.remove(name))
        .is_some();
    if !removed {
        return Ok(None);
    }
    Ok(Some(Plan {
        path: path.display().to_string(),
        after: render(&root),
        before_hash: hash(&before),
        before,
    }))
}

/// Write a plan the person approved: re-check, back up, write atomically.
pub fn apply(plan: &Plan, backups: &Path) -> Result<Applied, String> {
    let path = PathBuf::from(&plan.path);
    let current = read(&path)?.unwrap_or_default();
    if hash(&current) != plan.before_hash {
        return Err(format!(
            "{} changed since you reviewed it; nothing was written. Review it again.",
            path.display()
        ));
    }
    if plan.changes_nothing() {
        return Ok(Applied {
            backup: None,
            written_hash: hash(&current),
        });
    }

    let backup = if path.exists() {
        Some(back_up(&path, &current, backups)?)
    } else {
        None
    };

    let dir = path.parent().ok_or("no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(
        ".{}.whity-agent.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config")
    ));
    std::fs::write(&tmp, &plan.after).map_err(|e| format!("can't write {}: {e}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("can't replace {}: {e}", path.display()));
    }

    Ok(Applied {
        backup: backup.map(|b| b.display().to_string()),
        written_hash: hash(&plan.after),
    })
}

/// Put a backup back, if the file is still exactly what whity-agent wrote.
pub fn restore(
    path: &Path,
    backup: &Path,
    written_hash: &str,
    backups: &Path,
) -> Result<(), String> {
    let current = read(path)?.unwrap_or_default();
    if hash(&current) != written_hash {
        return Err(format!(
            "{} has changed since whity-agent wrote it, so the backup wasn't put back",
            path.display()
        ));
    }
    let saved =
        std::fs::read_to_string(backup).map_err(|e| format!("can't read the backup: {e}"))?;
    let plan = Plan {
        path: path.display().to_string(),
        before: current.clone(),
        after: saved,
        before_hash: hash(&current),
    };
    apply(&plan, backups).map(|_| ())
}

fn back_up(path: &Path, text: &str, backups: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    let target = backups.join(format!("{stamp}-{name}"));
    std::fs::write(&target, text).map_err(|e| format!("can't back up {}: {e}", path.display()))?;

    // Keep the newest few; the names sort by time.
    let mut all: Vec<PathBuf> = std::fs::read_dir(backups)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    all.sort();
    while all.len() > KEEP_BACKUPS {
        let _ = std::fs::remove_file(all.remove(0));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR: JsonEntry = JsonEntry {
        container: "mcpServers",
        url_key: "url",
        http_type: false,
    };
    const URL: &str = "https://app.whity.dev/mcp";

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "whity-agent-writer-{}",
            crate::oauth::random_token()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn it_adds_only_our_entry_and_keeps_the_rest() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(
            &file,
            r#"{"mcpServers":{"github":{"command":"gh-mcp"}},"theme":"dark"}"#,
        )
        .unwrap();

        let plan = plan_add(&file, CURSOR, "whity-example-bakery", URL).unwrap();
        let applied = apply(&plan, &dir.join("backups")).unwrap();

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["github"]["command"], "gh-mcp");
        assert_eq!(
            written["mcpServers"]["whity-example-bakery"],
            serde_json::json!({ "url": URL })
        );
        assert_eq!(written["theme"], "dark");
        assert!(!plan.after.to_lowercase().contains("authorization"));
        let backup = PathBuf::from(applied.backup.unwrap());
        assert_eq!(std::fs::read_to_string(backup).unwrap(), plan.before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_new_file_is_created_and_a_repeat_changes_nothing() {
        let dir = temp();
        let file = dir.join("sub").join("mcp.json");

        let plan = plan_add(&file, CURSOR, "whity", URL).unwrap();
        assert_eq!(plan.before, "");
        assert!(apply(&plan, &dir.join("backups")).unwrap().backup.is_none());
        assert!(plan_add(&file, CURSOR, "whity", URL)
            .unwrap()
            .changes_nothing());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_different_entry_of_the_same_name_is_left_alone() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(
            &file,
            r#"{"mcpServers":{"whity":{"url":"https://other.example/mcp"}}}"#,
        )
        .unwrap();

        assert!(plan_add(&file, CURSOR, "whity", URL)
            .unwrap_err()
            .contains("already has a different server"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_with_comments_is_left_alone() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(&file, "{\n  // mine\n  \"servers\": {}\n}\n").unwrap();

        assert!(plan_add(&file, CURSOR, "whity", URL)
            .unwrap_err()
            .contains("left alone"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "{\n  // mine\n  \"servers\": {}\n}\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_changed_after_review_is_not_written() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(&file, "{}").unwrap();
        let plan = plan_add(&file, CURSOR, "whity", URL).unwrap();

        std::fs::write(&file, r#"{"mcpServers":{}}"#).unwrap();

        assert!(apply(&plan, &dir.join("backups"))
            .unwrap_err()
            .contains("changed since you reviewed"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            r#"{"mcpServers":{}}"#
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn restore_puts_the_backup_back_only_over_our_own_write() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(&file, r#"{"a":1}"#).unwrap();
        let plan = plan_add(&file, CURSOR, "whity", URL).unwrap();
        let applied = apply(&plan, &dir.join("backups")).unwrap();
        let backup = PathBuf::from(applied.backup.clone().unwrap());

        restore(&file, &backup, &applied.written_hash, &dir.join("backups")).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"a":1}"#);

        // Now the file isn't what whity-agent wrote: no second restore over it.
        assert!(restore(&file, &backup, &applied.written_hash, &dir.join("backups")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_takes_out_only_our_entry() {
        let dir = temp();
        let file = dir.join("mcp.json");
        std::fs::write(
            &file,
            r#"{"mcpServers":{"whity":{"url":"x"},"github":{"command":"gh"}}}"#,
        )
        .unwrap();

        let plan = plan_remove(&file, "mcpServers", "whity").unwrap().unwrap();
        apply(&plan, &dir.join("backups")).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(written["mcpServers"].get("whity").is_none());
        assert_eq!(written["mcpServers"]["github"]["command"], "gh");
        assert!(plan_remove(&file, "mcpServers", "whity").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        let dir = temp();
        let backups = dir.join("backups");
        for i in 0..(KEEP_BACKUPS + 5) {
            std::fs::create_dir_all(&backups).unwrap();
            std::fs::write(backups.join(format!("{i:05}-old.json")), "{}").unwrap();
        }
        back_up(&dir.join("mcp.json"), "{}", &backups).unwrap();
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), KEEP_BACKUPS);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
