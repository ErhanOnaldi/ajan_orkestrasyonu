//! Daemon configuration and local-state paths (impl plan §8, spec §3.7).

use divan_core::TaskKind;
use std::path::PathBuf;

/// Kind-based watchdog defaults used when `tasks.max_runtime_secs` is NULL
/// (spec §3.7, impl plan §8.1 `[runtime_defaults]`).
#[derive(Debug, Clone, Copy)]
pub struct RuntimeDefaults {
    pub implement_secs: u32,
    pub review_secs: u32,
    pub test_secs: u32,
    /// Fallback for plan/research/analyze/report.
    pub other_secs: u32,
}

impl Default for RuntimeDefaults {
    fn default() -> Self {
        Self {
            implement_secs: 1800,
            review_secs: 900,
            test_secs: 1200,
            other_secs: 900,
        }
    }
}

impl RuntimeDefaults {
    pub fn for_kind(&self, kind: TaskKind) -> u32 {
        match kind {
            TaskKind::Implement => self.implement_secs,
            TaskKind::Review => self.review_secs,
            TaskKind::Test => self.test_secs,
            TaskKind::Plan | TaskKind::Research | TaskKind::Analyze | TaskKind::Report => {
                self.other_secs
            }
        }
    }
}

/// Daemon configuration. v1 targets macOS + Linux (unix socket; P0.4).
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub lock_path: PathBuf,
    pub artifact_root: PathBuf,
    pub worktree_root: PathBuf,
    pub runtime_defaults: RuntimeDefaults,
}

impl DaemonConfig {
    /// Default config rooted at `~/.local/share/divan` (impl plan §8.2). The
    /// per-repo artifact/worktree roots default to `.divan/...` under `cwd`.
    pub fn default_for_home(home: &std::path::Path) -> Self {
        let base = home.join(".local/share/divan");
        Self {
            socket_path: base.join("divan.sock"),
            db_path: base.join("divan.db"),
            lock_path: base.join("divan.lock"),
            artifact_root: PathBuf::from(".divan/artifacts"),
            worktree_root: PathBuf::from(".divan/worktrees"),
            runtime_defaults: RuntimeDefaults::default(),
        }
    }

    /// Resolve the effective runtime budget for a task (explicit value wins).
    pub fn runtime_secs(&self, kind: TaskKind, explicit: Option<u32>) -> u32 {
        explicit.unwrap_or_else(|| self.runtime_defaults.for_kind(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_runtime_overrides_default() {
        let cfg = DaemonConfig::default_for_home(std::path::Path::new("/home/x"));
        assert_eq!(cfg.runtime_secs(TaskKind::Review, None), 900);
        assert_eq!(cfg.runtime_secs(TaskKind::Review, Some(42)), 42);
        assert_eq!(cfg.runtime_secs(TaskKind::Implement, None), 1800);
        assert_eq!(cfg.runtime_secs(TaskKind::Plan, None), 900);
    }

    #[test]
    fn paths_under_home() {
        let cfg = DaemonConfig::default_for_home(std::path::Path::new("/home/x"));
        assert!(cfg.socket_path.ends_with("divan.sock"));
        assert!(cfg.lock_path.ends_with("divan.lock"));
    }
}
