//! `divan-daemon` — the hub daemon binary (impl plan §F1.3).
//!
//! Bootstrap order (spec §3.7): acquire the singleton lock, open the DB, run
//! startup reconciliation (`failed(orphaned)`), register the built-in adapters,
//! then serve the unix-socket JSON-RPC until shutdown.

use std::path::PathBuf;
use std::sync::Arc;

use divan_adapters::{
    agy::AgyAdapter, claude::ClaudeAdapter, codex::CodexAdapter, copilot::CopilotAdapter,
};
use divan_daemon::config::DaemonConfig;
use divan_daemon::lifecycle::{reconcile, LockFile};
use divan_daemon::router::CostRouter;
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

    // Make Divan's per-repo state dir (`.divan/`) git-ignored so it doesn't make
    // the target repo look "dirty" — otherwise `divan merge` would refuse on its
    // own artifacts/worktrees. Writes `<.divan>/.gitignore` with `*` once.
    if let Some(divan_dir) = config.artifact_root.parent() {
        if std::fs::create_dir_all(divan_dir).is_ok() {
            let gi = divan_dir.join(".gitignore");
            if !gi.exists() {
                let _ = std::fs::write(&gi, "*\n");
            }
        }
    }

    // Wire the scheduler with the built-in adapters (claude=writer, codex=reviewer).
    let artifacts = Arc::new(ArtifactStore::new(db.clone(), config.artifact_root.clone()));
    let worktrees = Arc::new(WorktreeManager::new(config.worktree_root.clone()));
    // Message bus (K3-K6) shares the same SQLite handle.
    let bus = divan_daemon::bus::MessageBus::new(db.clone());

    let mut scheduler = Scheduler::new(db, artifacts, worktrees, config.clone())
        .with_bus(bus.clone())
        .with_router(CostRouter::default_rules());
    // Built-in adapters (the §3.3 matrix). Copilot is the 3rd adapter (F3.4);
    // agy is the degraded one-shot 4th (F3.5, multi_turn=false).
    scheduler.register_adapter(Arc::new(ClaudeAdapter::new("claude-1")))?;
    scheduler.register_adapter(Arc::new(CodexAdapter::new("codex-1")))?;
    scheduler.register_adapter(Arc::new(CopilotAdapter::new("copilot-1")))?;
    scheduler.register_adapter(Arc::new(AgyAdapter::new("agy-1")))?;
    let scheduler = Arc::new(scheduler);

    // Serve until a `shutdown` RPC (or SIGINT/SIGTERM) arrives.
    let stop = Arc::new(tokio::sync::Notify::new());
    let signal_stop = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        signal_stop.notify_one();
    });

    rpc::serve(&config.socket_path, scheduler, bus, stop).await?;
    Ok(())
}
