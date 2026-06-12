//! `divan` — the user-facing CLI (impl plan §7, §F1.9). A thin client over the
//! daemon's unix-socket JSON-RPC (spec §3.1). v1 = macOS + Linux (P0.4).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use divan_daemon::config::DaemonConfig;
use divan_daemon::protocol::{method, RpcRequest, RpcResponse};
use divan_daemon::rpc;

#[derive(Parser)]
#[command(
    name = "divan",
    version,
    about = "Divan — local-first AI agent orchestration hub"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the hub daemon (no-op if one is already running).
    Up,
    /// Stop the running hub daemon.
    Down,
    /// Show agents and tasks.
    Status,
    /// Run a flow end-to-end (Faz 1: write-review).
    Run {
        /// The task description.
        task: String,
        #[arg(long, default_value = "write-review")]
        flow: String,
        /// Target git repository.
        #[arg(long)]
        repo: PathBuf,
    },
    /// Show the trace log (optionally filtered).
    Log {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        trace: Option<String>,
    },
    /// Show a task's worktree diff.
    Diff { task_id: String },
    /// Merge a task's worktree branch (explicit; never automatic).
    Merge { task_id: String },
    /// Remove a task's worktree (artifacts are kept).
    Cleanup { task_id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let config = DaemonConfig::default_for_home(&home);
    let sock = &config.socket_path;

    match cli.command {
        Command::Up => up(sock).await,
        Command::Down => {
            let r = call(sock, method::SHUTDOWN, serde_json::Value::Null).await?;
            print_result("daemon", &r);
            Ok(())
        }
        Command::Status => {
            let r = call(sock, method::STATUS, serde_json::Value::Null).await?;
            print_status(&r);
            Ok(())
        }
        Command::Run { task, flow, repo } => run(sock, task, flow, repo).await,
        Command::Log { task, trace } => {
            let params = serde_json::json!({ "task": task, "trace": trace });
            let r = call(sock, method::LOG, params).await?;
            print_log(&r);
            Ok(())
        }
        Command::Diff { task_id } => {
            let r = call(sock, method::DIFF, serde_json::json!({"task_id": task_id})).await?;
            if let Some(d) = r.result.get("diff").and_then(|v| v.as_str()) {
                print!("{d}");
            } else {
                print_result("diff", &r);
            }
            Ok(())
        }
        Command::Merge { task_id } => {
            let r = call(sock, method::MERGE, serde_json::json!({"task_id": task_id})).await?;
            print_result("merge", &r);
            Ok(())
        }
        Command::Cleanup { task_id } => {
            let r = call(
                sock,
                method::CLEANUP,
                serde_json::json!({"task_id": task_id}),
            )
            .await?;
            print_result("cleanup", &r);
            Ok(())
        }
    }
}

/// Start the daemon if the socket isn't already answering (impl plan §F1.3:
/// a second `divan up` connects to the existing daemon, never a second one).
async fn up(sock: &Path) -> Result<()> {
    if rpc::is_alive(sock).await {
        println!("divan: daemon already running");
        return Ok(());
    }
    let daemon_bin = daemon_binary_path()?;
    std::process::Command::new(&daemon_bin)
        .spawn()
        .with_context(|| format!("failed to start daemon at {}", daemon_bin.display()))?;

    // Wait for the socket to come up (~5s).
    for _ in 0..50 {
        if rpc::is_alive(sock).await {
            println!("divan: daemon started ({})", sock.display());
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("daemon did not become ready within 5s");
}

/// Locate the `divan-daemon` binary next to this `divan` binary.
fn daemon_binary_path() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().context("no parent dir for current exe")?;
    let cand = dir.join("divan-daemon");
    if cand.exists() {
        Ok(cand)
    } else {
        // Fall back to PATH resolution.
        Ok(PathBuf::from("divan-daemon"))
    }
}

async fn run(sock: &Path, task: String, flow: String, repo: PathBuf) -> Result<()> {
    if flow != "write-review" {
        bail!("unknown flow '{flow}' (Faz 1 supports: write-review)");
    }
    if !repo.join(".git").exists() {
        bail!("{} is not a git repository", repo.display());
    }
    let title: String = task.chars().take(60).collect();
    let params = serde_json::json!({
        "title": title,
        "spec": task,
        "repo": repo.to_string_lossy(),
    });
    println!("divan: running write-review on {} …", repo.display());
    let r = call(sock, method::RUN, params).await?;
    if r.ok {
        let rep = &r.result;
        println!("✓ flow complete");
        println!("  trace      {}", field(rep, "trace_id"));
        println!(
            "  implement  {}  (writer {})",
            field(rep, "implement_state"),
            field(rep, "writer")
        );
        println!(
            "  review     {}  (reviewer {})",
            field(rep, "review_state"),
            field(rep, "reviewer")
        );
        println!("  spec   artifact {}", field(rep, "spec_ref"));
        println!("  diff   artifact {}", field(rep, "diff_ref"));
        println!("  review artifact {}", field(rep, "review_ref"));
        println!("  report artifact {}", field(rep, "final_report_ref"));
    } else {
        eprintln!("✗ flow failed: {}", r.error.unwrap_or_default());
        std::process::exit(1);
    }
    Ok(())
}

async fn call(sock: &Path, method: &str, params: serde_json::Value) -> Result<RpcResponse> {
    if !rpc::is_alive(sock).await {
        bail!("divan daemon is not running — start it with `divan up`");
    }
    rpc::send(sock, &RpcRequest::new(method, params))
        .await
        .context("RPC call failed")
}

fn field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("-")
        .to_string()
}

fn print_result(label: &str, r: &RpcResponse) {
    if r.ok {
        println!("divan: {label} ok");
    } else {
        eprintln!(
            "divan: {label} failed: {}",
            r.error.clone().unwrap_or_default()
        );
    }
}

fn print_status(r: &RpcResponse) {
    if !r.ok {
        eprintln!(
            "divan: status failed: {}",
            r.error.clone().unwrap_or_default()
        );
        return;
    }
    println!("AGENTS");
    if let Some(agents) = r.result.get("agents").and_then(|v| v.as_array()) {
        for a in agents {
            println!(
                "  {:<12} {:<8} {:<8} cost={}",
                field(a, "id"),
                field(a, "tool"),
                field(a, "status"),
                a.get("cost_class").and_then(|c| c.as_u64()).unwrap_or(0)
            );
        }
    }
    println!("TASKS");
    if let Some(tasks) = r.result.get("tasks").and_then(|v| v.as_array()) {
        for t in tasks {
            println!(
                "  {:<14} {:<10} {:<9} {}",
                field(t, "id"),
                field(t, "kind"),
                field(t, "state"),
                t.get("assignee").and_then(|x| x.as_str()).unwrap_or("-")
            );
        }
    }
}

fn print_log(r: &RpcResponse) {
    if !r.ok {
        eprintln!("divan: log failed: {}", r.error.clone().unwrap_or_default());
        return;
    }
    if let Some(events) = r.result.get("events").and_then(|v| v.as_array()) {
        for e in events {
            let data = e.get("data").map(|d| d.to_string()).unwrap_or_default();
            println!(
                "  {:>14}  {:<22} {:<10} {}",
                e.get("ts").and_then(|t| t.as_i64()).unwrap_or(0),
                field(e, "event"),
                e.get("agent").and_then(|a| a.as_str()).unwrap_or("-"),
                if data == "null" { String::new() } else { data },
            );
        }
    }
}
