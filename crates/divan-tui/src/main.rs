//! `divan-tui` — the ratatui observability screen (impl plan §F4.2).
//!
//! Four panels over the daemon's unix-socket JSON-RPC: Agents, Tasks (the
//! navigable list), Messages (live flow), and Trace/detail (for the selected
//! task). It polls the daemon ~1s and redraws on tick or key. The data layer
//! ([`snapshot`]) and rendering ([`ui`]) are pure/testable without a TTY; a
//! `--once` headless mode renders a single frame to text for a smoke path.

mod snapshot;
mod ui;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEventKind};
use divan_daemon::config::DaemonConfig;
use divan_daemon::protocol::{method, RpcRequest};
use divan_daemon::rpc;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::snapshot::{parse_agents, parse_messages, parse_tasks, parse_trace, Snapshot};
use crate::ui::Focus;

const POLL_INTERVAL: Duration = Duration::from_millis(1000);

#[tokio::main]
async fn main() -> Result<()> {
    let once = std::env::args().any(|a| a == "--once" || a == "--dump");

    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let sock = DaemonConfig::default_for_home(&home).socket_path;

    if once {
        return run_once(&sock).await;
    }
    run_interactive(&sock).await
}

/// Headless smoke path: fetch one snapshot, render a single frame to a string
/// via `TestBackend`, print it, exit 0. If the daemon is down, print the
/// "daemon not running" line and still exit 0 (no TTY required, never panics).
async fn run_once(sock: &Path) -> Result<()> {
    let snap = fetch_snapshot(sock, None).await;
    if snap.daemon_down {
        println!("daemon not running (divan up)");
        return Ok(());
    }
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| ui::render(f, &snap, Focus::Tasks, 0))?;
    print!("{}", buffer_to_string(&terminal));
    Ok(())
}

/// Format a `TestBackend` terminal's buffer into newline-separated rows (used by
/// `--once` and the render tests; no TTY required).
fn buffer_to_string(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let width = buf.area().width as usize;
    buf.content()
        .iter()
        .map(|c| c.symbol())
        .collect::<Vec<_>>()
        .chunks(width.max(1))
        .map(|row| row.concat().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// In-memory UI state for the interactive loop.
struct App {
    snapshot: Snapshot,
    focus: Focus,
    /// Index into `snapshot.tasks`.
    selected: usize,
    /// The task id whose trace is currently loaded into the detail panel.
    loaded_trace: Option<String>,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            snapshot: Snapshot::default(),
            focus: Focus::Tasks,
            selected: 0,
            loaded_trace: None,
            should_quit: false,
        }
    }

    fn selected_task_id(&self) -> Option<String> {
        self.snapshot.tasks.get(self.selected).map(|t| t.id.clone())
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        let n = self.snapshot.tasks.len();
        if n > 0 && self.selected + 1 < n {
            self.selected += 1;
        }
    }
}

/// The interactive ratatui app. Uses `ratatui::init()` / `ratatui::restore()`
/// plus a panic-hook + drop guard so the terminal is always restored (raw mode
/// off, alternate screen left) even on panic.
///
/// Terminal input is read on a dedicated blocking thread (crossterm is
/// blocking) and forwarded over an mpsc channel, so the async loop can
/// `select!` between the ~1s poll tick and keypresses without extra
/// stream-adapter dependencies.
async fn run_interactive(sock: &Path) -> Result<()> {
    let _guard = TerminalGuard::install();
    let mut terminal = ratatui::init();

    let mut app = App::new();
    // Initial load so the screen isn't blank before the first tick.
    app.snapshot = fetch_snapshot(sock, app.loaded_trace.as_deref()).await;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let input = std::thread::spawn(move || input_loop(tx));

    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|f| ui::render(f, &app.snapshot, app.focus, app.selected))?;
        if app.should_quit {
            break;
        }

        tokio::select! {
            _ = poll.tick() => {
                app.snapshot = fetch_snapshot(sock, app.loaded_trace.as_deref()).await;
            }
            maybe_event = rx.recv() => {
                match maybe_event {
                    Some(event) => handle_event(&mut app, sock, event).await,
                    // Input thread ended (channel closed) — exit cleanly.
                    None => break,
                }
            }
        }
    }
    // Drop the receiver so the input thread's `send` fails and it returns.
    drop(rx);
    let _ = input.join();
    Ok(())
}

/// Blocking input reader: polls crossterm for events and forwards them. Returns
/// when the channel closes (UI loop exited) or on a read error.
fn input_loop(tx: tokio::sync::mpsc::UnboundedSender<Event>) {
    loop {
        match crossterm::event::poll(Duration::from_millis(200)) {
            Ok(true) => match crossterm::event::read() {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
            Ok(false) => {
                // No event within the poll window; keep looping unless the UI
                // has gone away.
                if tx.is_closed() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Handle one terminal event (key navigation). Non-key events are ignored.
async fn handle_event(app: &mut App, sock: &Path, event: Event) {
    let Event::Key(key) = event else { return };
    // crossterm sends Press + Release on some platforms; act on Press only.
    if key.kind == KeyEventKind::Release {
        return;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Tab => app.focus = app.focus.next(),
        KeyCode::Enter => {
            // Load the selected task's trace into the detail panel.
            if let Some(id) = app.selected_task_id() {
                app.loaded_trace = Some(id.clone());
                app.snapshot.trace = fetch_trace(sock, &id).await;
            }
        }
        _ => {}
    }
}

/// Fetch a full snapshot from the daemon. On any RPC failure (daemon down,
/// socket error, malformed response) returns [`Snapshot::daemon_down`] rather
/// than erroring — the screen never panics on RPC trouble.
async fn fetch_snapshot(sock: &Path, loaded_trace: Option<&str>) -> Snapshot {
    if !rpc::is_alive(sock).await {
        return Snapshot::daemon_down();
    }
    let status = match call(sock, method::STATUS, serde_json::Value::Null).await {
        Some(v) => v,
        None => return Snapshot::daemon_down(),
    };
    let agents = parse_agents(&status);
    let tasks = parse_tasks(&status);

    let messages = call(sock, method::MESSAGES, serde_json::json!({}))
        .await
        .map(|v| parse_messages(&v))
        .unwrap_or_default();

    // Refresh the loaded task's trace if one is selected (so the detail panel
    // stays live across ticks).
    let trace = match loaded_trace {
        Some(id) => fetch_trace(sock, id).await,
        None => snapshot::TraceDetail::default(),
    };

    Snapshot {
        agents,
        tasks,
        messages,
        trace,
        daemon_down: false,
    }
}

/// Fetch + parse a single task's trace. Failures yield an empty detail (with the
/// task id retained) instead of crashing.
async fn fetch_trace(sock: &Path, task_id: &str) -> snapshot::TraceDetail {
    match call(sock, method::TRACE, serde_json::json!({ "id": task_id })).await {
        Some(v) => parse_trace(task_id, &v),
        None => snapshot::TraceDetail {
            task_id: task_id.to_string(),
            ..Default::default()
        },
    }
}

/// Send one RPC; return the `result` value on success, `None` on any failure
/// (transport error or `ok == false`).
async fn call(sock: &Path, method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    match rpc::send(sock, &RpcRequest::new(method, params)).await {
        Ok(resp) if resp.ok => Some(resp.result),
        _ => None,
    }
}

/// Restores the terminal (leave alternate screen, disable raw mode) on drop AND
/// installs a panic hook that does the same, so a panic mid-render never leaves
/// the user's terminal wedged.
struct TerminalGuard;

impl TerminalGuard {
    fn install() -> Self {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            ratatui::restore();
            original(info);
        }));
        TerminalGuard
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_tasks(n: usize) -> App {
        let mut app = App::new();
        app.snapshot.tasks = (0..n)
            .map(|i| snapshot::TaskRow {
                id: format!("t-{i}"),
                kind: "implement".into(),
                state: "open".into(),
                assignee: "-".into(),
            })
            .collect();
        app
    }

    #[test]
    fn navigation_clamps_at_bounds() {
        let mut app = app_with_tasks(3);
        assert_eq!(app.selected, 0);
        app.move_up(); // stays at 0
        assert_eq!(app.selected, 0);
        app.move_down();
        app.move_down();
        assert_eq!(app.selected, 2);
        app.move_down(); // stays at last
        assert_eq!(app.selected, 2);
        assert_eq!(app.selected_task_id().as_deref(), Some("t-2"));
    }

    #[test]
    fn navigation_safe_with_no_tasks() {
        let mut app = app_with_tasks(0);
        app.move_down();
        app.move_up();
        assert_eq!(app.selected, 0);
        assert_eq!(app.selected_task_id(), None);
    }

    #[test]
    fn tab_cycles_focus() {
        let mut app = App::new();
        assert_eq!(app.focus, Focus::Tasks);
        app.focus = app.focus.next();
        app.focus = app.focus.next();
        app.focus = app.focus.next();
        app.focus = app.focus.next();
        assert_eq!(app.focus, Focus::Tasks);
    }
}
