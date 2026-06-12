//! Claude `settings.json` hook merge/unmerge.
//!
//! Shape proven by spike S3 (`docs/spikes/s3_hook_injection.md` §2):
//!
//! ```json
//! { "hooks": {
//!   "UserPromptSubmit": [ { "hooks": [ { "type":"command", "command":"…" } ] } ],
//!   "Stop":            [ { "hooks": [ { "type":"command", "command":"…" } ] } ]
//! }}
//! ```
//!
//! Divan registers its `divan-turn-end.sh` under `UserPromptSubmit` (the
//! injection channel) and `divan-activity.sh` under `Stop` (activity/idle wake).
//!
//! Merge rules (F2.1): preserve every existing key and hook entry, add Divan's
//! entries idempotently (a Divan command is matched by its script basename, so a
//! re-run never duplicates), and on uninstall remove ONLY Divan's entries while
//! leaving user entries — pruning empty containers we created.

use std::path::Path;

use serde_json::{Map, Value};

/// Errors surfacing from settings JSON manipulation.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("settings `{0}` is not a JSON object")]
    NotAnObject(&'static str),
}

const TURN_END_EVENT: &str = "UserPromptSubmit";
const ACTIVITY_EVENT: &str = "Stop";

/// Basenames that identify a Divan-owned hook command (for idempotency + clean
/// uninstall). Any `command` ending with one of these is "ours".
const DIVAN_BASENAMES: &[&str] = &["divan-turn-end.sh", "divan-activity.sh"];

fn is_divan_command(cmd: &str) -> bool {
    DIVAN_BASENAMES.iter().any(|b| cmd.ends_with(b))
}

/// Merge Divan's two hook entries into `root`, idempotently. Returns the new root.
pub fn merge_install(
    mut root: Value,
    turn_end_path: &Path,
    activity_path: &Path,
) -> Result<Value, SettingsError> {
    if root.is_null() {
        root = Value::Object(Map::new());
    }
    let obj = root
        .as_object_mut()
        .ok_or(SettingsError::NotAnObject("root"))?;

    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or(SettingsError::NotAnObject("hooks"))?;

    add_command_entry(hooks, TURN_END_EVENT, &turn_end_path.to_string_lossy())?;
    add_command_entry(hooks, ACTIVITY_EVENT, &activity_path.to_string_lossy())?;

    Ok(root)
}

/// Append `{ "hooks": [ { "type":"command", "command": cmd } ] }` under `event`,
/// unless a Divan entry for that script basename already exists (idempotent).
fn add_command_entry(
    hooks: &mut Map<String, Value>,
    event: &str,
    command: &str,
) -> Result<(), SettingsError> {
    let arr = hooks
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let arr = arr
        .as_array_mut()
        .ok_or(SettingsError::NotAnObject("hooks event array"))?;

    // Idempotency: if any existing inner hook command is the same Divan script,
    // do nothing.
    let already_present = arr.iter().any(|matcher| {
        matcher
            .get("hooks")
            .and_then(Value::as_array)
            .map(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .map(|c| c == command)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    });
    if already_present {
        return Ok(());
    }

    arr.push(serde_json::json!({
        "hooks": [ { "type": "command", "command": command } ]
    }));
    Ok(())
}

/// Remove all Divan-owned hook entries from `root`, leaving user entries intact.
/// Prunes empty event arrays / the `hooks` object if they become empty *and*
/// were left empty solely by our removal.
pub fn merge_uninstall(mut root: Value) -> Result<Value, SettingsError> {
    let obj = match root.as_object_mut() {
        Some(o) => o,
        None => return Ok(root),
    };

    let hooks = match obj.get_mut("hooks").and_then(Value::as_object_mut) {
        Some(h) => h,
        None => return Ok(root),
    };

    for event in [TURN_END_EVENT, ACTIVITY_EVENT] {
        if let Some(arr) = hooks.get_mut(event).and_then(Value::as_array_mut) {
            strip_divan_from_event(arr);
        }
    }

    // Prune event keys that we emptied out.
    let empty_events: Vec<String> = hooks
        .iter()
        .filter(|(_, v)| v.as_array().map(|a| a.is_empty()).unwrap_or(false))
        .map(|(k, _)| k.clone())
        .collect();
    for k in empty_events {
        hooks.remove(&k);
    }

    let hooks_empty = hooks.is_empty();
    if hooks_empty {
        obj.remove("hooks");
    }

    Ok(root)
}

/// In one event array, drop matcher entries that contain Divan commands; if a
/// matcher mixes Divan + user commands, keep only the user commands.
fn strip_divan_from_event(arr: &mut Vec<Value>) {
    arr.retain_mut(|matcher| {
        let Some(inner) = matcher.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true; // not the command shape we own; leave untouched
        };
        inner.retain(|h| {
            !h.get("command")
                .and_then(Value::as_str)
                .map(is_divan_command)
                .unwrap_or(false)
        });
        // Drop the whole matcher only if it became empty (was purely ours).
        !inner.is_empty()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn merge_then_uninstall_roundtrips_to_user_only() {
        let user = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "/u/user.sh" } ] }
                ]
            }
        });
        let te = PathBuf::from("/cfg/divan/divan-turn-end.sh");
        let ac = PathBuf::from("/cfg/divan/divan-activity.sh");
        let merged = merge_install(user.clone(), &te, &ac).unwrap();
        let back = merge_uninstall(merged).unwrap();
        assert_eq!(back, user, "uninstall restores exact user-only config");
    }

    #[test]
    fn merge_from_empty_creates_hooks_object() {
        let te = PathBuf::from("/d/divan-turn-end.sh");
        let ac = PathBuf::from("/d/divan-activity.sh");
        let merged = merge_install(Value::Null, &te, &ac).unwrap();
        assert!(merged["hooks"]["UserPromptSubmit"].is_array());
        assert!(merged["hooks"]["Stop"].is_array());
        // Uninstall back to empty object (no stray hooks key).
        let back = merge_uninstall(merged).unwrap();
        assert_eq!(back, serde_json::json!({}));
    }

    #[test]
    fn mixed_matcher_keeps_user_command() {
        // A single matcher that contains both a user and a divan command.
        let cfg = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [
                        { "type": "command", "command": "/u/user-stop.sh" },
                        { "type": "command", "command": "/d/divan-activity.sh" }
                    ] }
                ]
            }
        });
        let back = merge_uninstall(cfg).unwrap();
        let inner = back["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], "/u/user-stop.sh");
    }
}
