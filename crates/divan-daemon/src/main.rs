//! `divan-daemon` — the hub daemon binary (impl plan §F1.3).
//!
//! Bootstrap order (spec §3.7): acquire the singleton lock, open the DB, run
//! startup reconciliation (`failed(orphaned)`), register the built-in adapters,
//! then serve the unix-socket JSON-RPC until shutdown.

use std::path::PathBuf;
use std::sync::Arc;

use divan_adapters::{claude::ClaudeAdapter, codex::CodexAdapter};
use divan_daemon::config::DaemonConfig;
use divan_daemon::lifecycle::{reconcile, LockFile};
use divan_daemon::scheduler::Scheduler;
use divan_daemon::{rpc, worktree::WorktreeManager};
use divan_db::{ArtifactStore, Db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "divan_daemon=info".into()),
        )
        .init();

    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let config = DaemonConfig::default_for_home(&home);

    // Singleton lock (spec §3.7). Held for the daemon's lifetime.
    let _lock = match LockFile::acquire(&config.lock_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("divan-daemon: {e}");
            std::process::exit(1);
        }
    };

    // DB + startup reconciliation.
    if let Some(parent) = config.db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Db::open(&config.db_path)?;
    let orphaned = reconcile(&db)?;
    if orphaned > 0 {
        tracing::warn!(orphaned, "reconciliation marked orphaned tasks failed");
    }

    // Wire the scheduler with the built-in adapters (claude=writer, codex=reviewer).
    let artifacts = Arc::new(ArtifactStore::new(db.clone(), config.artifact_root.clone()));
    let worktrees = Arc::new(WorktreeManager::new(config.worktree_root.clone()));
    let mut scheduler = Scheduler::new(db, artifacts, worktrees, config.clone());
    scheduler.register_adapter(Arc::new(ClaudeAdapter::new("claude-1")))?;
    scheduler.register_adapter(Arc::new(CodexAdapter::new("codex-1")))?;
    let scheduler = Arc::new(scheduler);

    // Serve until a `shutdown` RPC (or SIGINT/SIGTERM) arrives.
    let stop = Arc::new(tokio::sync::Notify::new());
    let signal_stop = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_stop.notify_one();
    });

    rpc::serve(&config.socket_path, scheduler, stop).await?;
    Ok(())
}
