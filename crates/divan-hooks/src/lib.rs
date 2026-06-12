//! `divan-hooks` — hook script generation + installer (Phase 2, F2.1/F2.2).
//!
//! This crate is a **library** (no `main`). It exposes an installer API the CLI
//! calls to wire Divan's turn-boundary injection + activity hooks into a
//! supported tool's config, MERGING with the user's existing hook config and
//! taking a BACKUP first — never overwriting.
//!
//! ## Proven mechanism (spike S3)
//! Claude Code hooks deliver Divan's two delivery primitives:
//! - `UserPromptSubmit` hook stdout (exit 0) is appended to context at the turn
//!   boundary → this is the **injection** channel (`divan-turn-end.sh`).
//! - `Stop` hook fires at turn/session end → **activity + idle-wake** signal
//!   (`divan-activity.sh`).
//!
//! All real logic lives in the daemon; the hook scripts are thin and do a single
//! JSON-RPC round-trip over the daemon's unix socket. When the daemon is down the
//! scripts exit 0 silently so the agent tool is never broken.
//!
//! ## Codex
//! Phase 0 spikes (S2/S5) verified codex's `--json` streaming and `exec resume`
//! but did NOT validate any rich hook injection mechanism analogous to Claude's
//! `UserPromptSubmit`. Per the architecture's delivery hierarchy (§3.3/§3.6),
//! codex delivery is **MCP-only**. `install`/`uninstall` for [`HookTool::Codex`]
//! are therefore a documented no-op (reported in `skipped`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod settings;

use settings::SettingsError;

/// Tool whose config we install Divan hooks into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTool {
    /// Claude Code — full hook support (`UserPromptSubmit` + `Stop`), per S3.
    Claude,
    /// Codex — MCP-only delivery (no rich hook system per Phase 0). No-op.
    Codex,
}

impl HookTool {
    /// Stable lowercase identifier (e.g. for CLI flag echoing / logging).
    pub fn label(self) -> &'static str {
        match self {
            HookTool::Claude => "claude",
            HookTool::Codex => "codex",
        }
    }
}

/// Outcome of an [`install`] call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    /// Human-readable identifiers of what was installed (hook scripts + entries).
    pub installed: Vec<String>,
    /// Path of the settings backup taken before modifying (None if nothing to back up).
    pub backup_path: Option<PathBuf>,
    /// Human-readable notes about anything intentionally skipped.
    pub skipped: Vec<String>,
}

/// Errors from installing/uninstalling Divan hooks.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("settings file {path} is not valid JSON: {source}")]
    Settings {
        path: PathBuf,
        #[source]
        source: SettingsError,
    },
}

impl HookError {
    fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        HookError::Io {
            path: path.into(),
            source,
        }
    }
}

/// Default daemon unix socket path the generated hook scripts contact.
pub const DEFAULT_SOCKET_REL: &str = ".local/share/divan/divan.sock";

/// Resolve the default config dir for a tool (e.g. `~/.claude`). Tests pass an
/// explicit dir instead of relying on this.
pub fn default_config_dir(tool: HookTool) -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    match tool {
        HookTool::Claude => home.join(".claude"),
        HookTool::Codex => home.join(".codex"),
    }
}

/// Resolve the default daemon socket path (`~/.local/share/divan/divan.sock`).
pub fn default_socket_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(DEFAULT_SOCKET_REL)
}

// --- Hook script templates (embedded; thin shells, logic lives in daemon) ---

const TURN_END_TMPL: &str = include_str!("../scripts/divan-turn-end.sh.tmpl");
const ACTIVITY_TMPL: &str = include_str!("../scripts/divan-activity.sh.tmpl");

const TURN_END_SCRIPT: &str = "divan-turn-end.sh";
const ACTIVITY_SCRIPT: &str = "divan-activity.sh";

/// Render a hook script template with the agent id + socket path substituted.
fn render_script(tmpl: &str, agent_id: &str, socket_path: &Path) -> String {
    tmpl.replace("__DIVAN_AGENT_ID__", agent_id)
        .replace("__DIVAN_SOCKET__", &socket_path.to_string_lossy())
}

/// Install Divan hooks for `tool` into `config_dir`, contacting the default
/// daemon socket. See [`install_with_socket`] to override the socket (tests).
pub fn install(
    tool: HookTool,
    config_dir: &Path,
    agent_id: &str,
) -> Result<InstallReport, HookError> {
    install_with_socket(
        tool,
        config_dir,
        agent_id,
        &default_socket_path(),
        "divan-bak",
    )
}

/// Install with explicit socket path + backup suffix (so tests are deterministic
/// and never read the real clock). The backup file is `settings.json.<suffix>`.
pub fn install_with_socket(
    tool: HookTool,
    config_dir: &Path,
    agent_id: &str,
    socket_path: &Path,
    backup_suffix: &str,
) -> Result<InstallReport, HookError> {
    match tool {
        HookTool::Claude => install_claude(config_dir, agent_id, socket_path, backup_suffix),
        HookTool::Codex => Ok(InstallReport {
            installed: Vec::new(),
            backup_path: None,
            skipped: vec![
                "codex: MCP-only delivery (no rich hook system per Phase 0 S2/S5); \
                 no hooks installed"
                    .to_string(),
            ],
        }),
    }
}

fn install_claude(
    config_dir: &Path,
    agent_id: &str,
    socket_path: &Path,
    backup_suffix: &str,
) -> Result<InstallReport, HookError> {
    let mut report = InstallReport::default();

    // 1. Write hook scripts into <config_dir>/divan/ and chmod +x.
    let script_dir = config_dir.join("divan");
    fs::create_dir_all(&script_dir).map_err(|e| HookError::io(&script_dir, e))?;

    let turn_end_path = script_dir.join(TURN_END_SCRIPT);
    let activity_path = script_dir.join(ACTIVITY_SCRIPT);

    write_script(
        &turn_end_path,
        &render_script(TURN_END_TMPL, agent_id, socket_path),
    )?;
    write_script(
        &activity_path,
        &render_script(ACTIVITY_TMPL, agent_id, socket_path),
    )?;
    report
        .installed
        .push(turn_end_path.to_string_lossy().into_owned());
    report
        .installed
        .push(activity_path.to_string_lossy().into_owned());

    // 2/3. Back up settings.json, then merge Divan's hook entries idempotently.
    let settings_path = config_dir.join("settings.json");
    let original = read_settings(&settings_path)?;

    if settings_path.exists() {
        let backup_path = settings_path.with_file_name(format!("settings.json.{backup_suffix}"));
        fs::copy(&settings_path, &backup_path).map_err(|e| HookError::io(&backup_path, e))?;
        report.backup_path = Some(backup_path);
    }

    let merged =
        settings::merge_install(original, &turn_end_path, &activity_path).map_err(|e| {
            HookError::Settings {
                path: settings_path.clone(),
                source: e,
            }
        })?;
    write_settings(&settings_path, &merged)?;

    report.installed.push(format!(
        "claude hooks merged into {}",
        settings_path.display()
    ));

    Ok(report)
}

/// Remove Divan's hooks for `tool` from `config_dir`. Idempotent.
pub fn uninstall(tool: HookTool, config_dir: &Path) -> Result<(), HookError> {
    match tool {
        HookTool::Claude => uninstall_claude(config_dir),
        HookTool::Codex => Ok(()),
    }
}

fn uninstall_claude(config_dir: &Path) -> Result<(), HookError> {
    let settings_path = config_dir.join("settings.json");
    if settings_path.exists() {
        let original = read_settings(&settings_path)?;
        let cleaned = settings::merge_uninstall(original).map_err(|e| HookError::Settings {
            path: settings_path.clone(),
            source: e,
        })?;
        write_settings(&settings_path, &cleaned)?;
    }

    let script_dir = config_dir.join("divan");
    if script_dir.exists() {
        fs::remove_dir_all(&script_dir).map_err(|e| HookError::io(&script_dir, e))?;
    }
    Ok(())
}

// --- small fs helpers ---

fn write_script(path: &Path, body: &str) -> Result<(), HookError> {
    fs::write(path, body).map_err(|e| HookError::io(path, e))?;
    set_executable(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), HookError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|e| HookError::io(path, e))?;
    let mut perms = meta.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms).map_err(|e| HookError::io(path, e))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), HookError> {
    // v1 targets macOS + Linux only; nothing to do off-unix.
    Ok(())
}

fn read_settings(path: &Path) -> Result<serde_json::Value, HookError> {
    if !path.exists() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    let raw = fs::read_to_string(path).map_err(|e| HookError::io(path, e))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(trimmed).map_err(|e| HookError::Settings {
        path: path.to_path_buf(),
        source: SettingsError::Json(e),
    })
}

fn write_settings(path: &Path, value: &serde_json::Value) -> Result<(), HookError> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| HookError::Settings {
        path: path.to_path_buf(),
        source: SettingsError::Json(e),
    })?;
    text.push('\n');
    fs::write(path, text).map_err(|e| HookError::io(path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[cfg(unix)]
    fn is_executable(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).unwrap().permissions().mode();
        mode & 0o111 != 0
    }

    /// install into a dir that ALREADY has a user hook entry: user entry must
    /// survive, Divan's entry is added, a backup exists, scripts exist + exec.
    #[test]
    fn install_merges_and_preserves_user_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        let settings_path = cfg.join("settings.json");

        // Pre-existing user config with a user hook + an unrelated top-level key.
        let user = serde_json::json!({
            "model": "claude-opus-4-8",
            "hooks": {
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "/usr/local/bin/my-user-hook.sh" } ] }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string_pretty(&user).unwrap()).unwrap();

        let socket = PathBuf::from("/tmp/divan-test.sock");
        let report =
            install_with_socket(HookTool::Claude, cfg, "agent-1", &socket, "divan-bak").unwrap();

        // Backup taken.
        let backup = report.backup_path.expect("backup taken");
        assert!(backup.exists(), "backup file must exist");
        assert_eq!(read_json(&backup), user, "backup is the verbatim original");

        // Scripts written + executable, with substitutions applied.
        let turn_end = cfg.join("divan").join(TURN_END_SCRIPT);
        let activity = cfg.join("divan").join(ACTIVITY_SCRIPT);
        assert!(turn_end.exists() && activity.exists());
        #[cfg(unix)]
        {
            assert!(is_executable(&turn_end));
            assert!(is_executable(&activity));
        }
        let te_body = fs::read_to_string(&turn_end).unwrap();
        assert!(te_body.contains("agent-1"), "agent id substituted");
        assert!(
            te_body.contains("/tmp/divan-test.sock"),
            "socket path substituted"
        );
        assert!(
            !te_body.contains("__DIVAN_AGENT_ID__"),
            "no placeholder left"
        );

        // Settings merged: user top-level key + user hook preserved, Divan added.
        let merged = read_json(&settings_path);
        assert_eq!(merged["model"], "claude-opus-4-8", "unrelated key kept");
        let ups = merged["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let cmds: Vec<&str> = ups
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert!(
            cmds.iter().any(|c| c.contains("my-user-hook.sh")),
            "user hook survives"
        );
        assert!(
            cmds.iter().any(|c| c.ends_with("divan-turn-end.sh")),
            "divan turn-end hook added"
        );
        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        let stop_cmds: Vec<&str> = stop
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert!(
            stop_cmds.iter().any(|c| c.ends_with("divan-activity.sh")),
            "divan activity hook added to Stop"
        );
    }

    /// install twice -> idempotent, no duplicate Divan entry.
    #[test]
    fn install_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        let socket = PathBuf::from("/tmp/divan-test.sock");

        install_with_socket(HookTool::Claude, cfg, "a", &socket, "bak1").unwrap();
        install_with_socket(HookTool::Claude, cfg, "a", &socket, "bak2").unwrap();

        let merged = read_json(&cfg.join("settings.json"));
        let ups = merged["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let divan_count = ups
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .filter(|h| {
                h["command"]
                    .as_str()
                    .map(|c| c.ends_with("divan-turn-end.sh"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(divan_count, 1, "no duplicate divan turn-end entry");

        let stop = merged["hooks"]["Stop"].as_array().unwrap();
        let stop_count = stop
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .filter(|h| {
                h["command"]
                    .as_str()
                    .map(|c| c.ends_with("divan-activity.sh"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(stop_count, 1, "no duplicate divan activity entry");
    }

    /// uninstall -> Divan entries gone, user entries restored, divan/ removed.
    #[test]
    fn uninstall_restores_user_and_removes_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        let settings_path = cfg.join("settings.json");
        let user = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [
                    { "hooks": [ { "type": "command", "command": "/usr/local/bin/my-user-hook.sh" } ] }
                ],
                "Stop": [
                    { "hooks": [ { "type": "command", "command": "/usr/local/bin/user-stop.sh" } ] }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string_pretty(&user).unwrap()).unwrap();

        let socket = PathBuf::from("/tmp/divan-test.sock");
        install_with_socket(HookTool::Claude, cfg, "a", &socket, "bak").unwrap();
        uninstall(HookTool::Claude, cfg).unwrap();

        // divan/ dir gone.
        assert!(!cfg.join("divan").exists(), "divan script dir removed");

        // Settings: user entries restored, no divan entries remain.
        let after = read_json(&settings_path);
        let ups = after["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let ups_cmds: Vec<&str> = ups
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert!(ups_cmds.iter().any(|c| c.contains("my-user-hook.sh")));
        assert!(
            !ups_cmds.iter().any(|c| c.contains("divan")),
            "no divan entry left"
        );
        let stop = after["hooks"]["Stop"].as_array().unwrap();
        let stop_cmds: Vec<&str> = stop
            .iter()
            .flat_map(|m| m["hooks"].as_array().unwrap())
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert!(stop_cmds.iter().any(|c| c.contains("user-stop.sh")));
        assert!(!stop_cmds.iter().any(|c| c.contains("divan")));
    }

    /// uninstall on a clean dir is a harmless no-op (idempotent).
    #[test]
    fn uninstall_is_idempotent_on_clean_dir() {
        let dir = tempfile::tempdir().unwrap();
        uninstall(HookTool::Claude, dir.path()).unwrap();
        uninstall(HookTool::Claude, dir.path()).unwrap();
    }

    /// Generated scripts carry the no-op-when-down guard and exit 0.
    #[test]
    fn generated_scripts_have_noop_guard() {
        let socket = PathBuf::from("/tmp/divan-test.sock");
        for tmpl in [TURN_END_TMPL, ACTIVITY_TMPL] {
            let body = render_script(tmpl, "agent-x", &socket);
            assert!(body.contains("DIVAN_FAKE_NO_SOCKET"), "test seam present");
            assert!(
                body.contains("[ ! -S \"$DIVAN_SOCKET\" ]"),
                "socket-existence guard present"
            );
            // The guard body exits 0 (silent no-op) when the socket is absent.
            assert!(body.contains("exit 0"), "no-op exit 0 present");
        }
    }

    /// codex install is a documented no-op (MCP-only), nothing written.
    #[test]
    fn codex_install_is_noop_with_note() {
        let dir = tempfile::tempdir().unwrap();
        let report =
            install_with_socket(HookTool::Codex, dir.path(), "a", Path::new("/tmp/s"), "bak")
                .unwrap();
        assert!(report.installed.is_empty());
        assert!(report.backup_path.is_none());
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].contains("MCP-only"));
        assert!(!dir.path().join("divan").exists());
        // uninstall is also a no-op.
        uninstall(HookTool::Codex, dir.path()).unwrap();
    }

    /// If the generated script actually runs under a POSIX shell with the
    /// no-socket seam set, it must exit 0 and print nothing.
    #[cfg(unix)]
    #[test]
    fn generated_turn_end_script_noop_runs_clean() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path();
        // Socket path points at a non-existent socket on purpose.
        let socket = cfg.join("absent.sock");
        install_with_socket(HookTool::Claude, cfg, "agent-x", &socket, "bak").unwrap();
        let script = cfg.join("divan").join(TURN_END_SCRIPT);

        let out = Command::new("/bin/sh")
            .arg(&script)
            .env("DIVAN_FAKE_NO_SOCKET", "1")
            .output()
            .expect("run hook script");
        assert!(out.status.success(), "no-op exits 0");
        assert!(out.stdout.is_empty(), "no-op prints nothing to context");
    }
}
