//! Rendering for the four-panel observability screen (impl plan §F4.2).
//!
//! Layout (top row split in two, bottom row split in two):
//! ```text
//! +------------------+-------------------------+
//! |     Agents       |        Tasks (nav)      |
//! +------------------+-------------------------+
//! |    Messages      |      Trace / detail     |
//! +------------------+-------------------------+
//! |              status / key bar               |
//! +---------------------------------------------+
//! ```
//! Rendering is a pure function of (`Snapshot`, `Focus`, selected index), so the
//! same code path drives the live terminal and the `--once` `TestBackend` smoke
//! test (no TTY required).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::snapshot::Snapshot;

/// Which panel currently has focus (Tab cycles through these).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tasks,
    Agents,
    Messages,
    Trace,
}

impl Focus {
    /// Cycle focus: Tasks -> Agents -> Messages -> Trace -> Tasks.
    pub fn next(self) -> Self {
        match self {
            Focus::Tasks => Focus::Agents,
            Focus::Agents => Focus::Messages,
            Focus::Messages => Focus::Trace,
            Focus::Trace => Focus::Tasks,
        }
    }
}

fn block(title: &str, focused: bool) -> Block<'_> {
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Span::styled(format!(" {title} "), style))
}

/// Render the whole screen into `frame` for the given state.
///
/// `selected` is the index into `snapshot.tasks` (the navigable list).
pub fn render(frame: &mut Frame, snapshot: &Snapshot, focus: Focus, selected: usize) {
    let area = frame.area();

    // Outer: two content rows + a one-line key bar.
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    if snapshot.daemon_down {
        render_daemon_down(frame, outer[0]);
        render_keybar(frame, outer[2]);
        // Still draw empty bottom panels so the layout is stable.
        let bottom = split_h(outer[1]);
        frame.render_widget(block("Messages", false), bottom[0]);
        frame.render_widget(block("Trace / detail", focus == Focus::Trace), bottom[1]);
        return;
    }

    let top = split_h(outer[0]);
    let bottom = split_h(outer[1]);

    render_agents(frame, top[0], snapshot, focus == Focus::Agents);
    render_tasks(frame, top[1], snapshot, focus == Focus::Tasks, selected);
    render_messages(frame, bottom[0], snapshot, focus == Focus::Messages);
    render_trace(frame, bottom[1], snapshot, focus == Focus::Trace);
    render_keybar(frame, outer[2]);
}

fn split_h(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area)
}

fn render_daemon_down(frame: &mut Frame, area: Rect) {
    let para = Paragraph::new(vec![
        Line::from(Span::styled(
            "daemon not running (divan up)",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Start the hub with `divan up`, then this screen will populate."),
        Line::from("Press q to quit."),
    ])
    .wrap(Wrap { trim: true })
    .block(block("Divan", false));
    frame.render_widget(para, area);
}

fn render_agents(frame: &mut Frame, area: Rect, snapshot: &Snapshot, focused: bool) {
    let items: Vec<ListItem> = if snapshot.agents.is_empty() {
        vec![ListItem::new("(no agents)")]
    } else {
        snapshot
            .agents
            .iter()
            .map(|a| {
                let task = a.current_task.as_deref().unwrap_or("-");
                ListItem::new(format!(
                    "{:<12} {:<8} {:<7} c{} @{}",
                    a.id, a.tool, a.status, a.cost_class, task
                ))
            })
            .collect()
    };
    frame.render_widget(List::new(items).block(block("Agents", focused)), area);
}

fn render_tasks(
    frame: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    focused: bool,
    selected: usize,
) {
    let items: Vec<ListItem> = if snapshot.tasks.is_empty() {
        vec![ListItem::new("(no tasks)")]
    } else {
        snapshot
            .tasks
            .iter()
            .map(|t| {
                // Show dependency edges (the task-tree/deps view, §F4.2).
                let deps = if t.deps.is_empty() {
                    String::new()
                } else {
                    format!(" deps[{}]", t.deps.join(","))
                };
                ListItem::new(format!(
                    "{:<16} {:<10} {:<9} {}{}",
                    t.id, t.kind, t.state, t.assignee, deps
                ))
            })
            .collect()
    };
    let list = List::new(items)
        .block(block("Tasks", focused))
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    if !snapshot.tasks.is_empty() {
        state.select(Some(selected.min(snapshot.tasks.len() - 1)));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_messages(frame: &mut Frame, area: Rect, snapshot: &Snapshot, focused: bool) {
    let items: Vec<ListItem> = if snapshot.messages.is_empty() {
        vec![ListItem::new("(no messages)")]
    } else {
        snapshot
            .messages
            .iter()
            .map(|m| {
                let mark = if m.delivered { "✓" } else { "·" };
                ListItem::new(format!(
                    "{} {:<8} {:<10} -> {:<10} {}",
                    mark, m.kind, m.from, m.to, m.summary
                ))
            })
            .collect()
    };
    frame.render_widget(List::new(items).block(block("Messages", focused)), area);
}

fn render_trace(frame: &mut Frame, area: Rect, snapshot: &Snapshot, focused: bool) {
    let d = &snapshot.trace;
    let title = if d.task_id.is_empty() {
        "Trace / detail".to_string()
    } else {
        format!("Trace / detail — {} ({} events)", d.task_id, d.event_count)
    };
    let mut lines: Vec<Line> = Vec::new();
    if d.task_id.is_empty() {
        lines.push(Line::from(
            "Select a task and press Enter to load its trace.",
        ));
    } else if d.text.trim().is_empty() {
        lines.push(Line::from("(no trace events yet)"));
    } else {
        for l in d.text.lines() {
            lines.push(Line::from(l.to_string()));
        }
    }
    if !d.metrics_line.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "METRICS (proxy measures; no % savings claim)",
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(d.metrics_line.clone()));
    }
    let para = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block(&title, focused));
    frame.render_widget(para, area);
}

fn render_keybar(frame: &mut Frame, area: Rect) {
    let bar = Paragraph::new(Line::from(vec![Span::styled(
        " [Up/Down or j/k] move  [Enter] load trace  [Tab] focus  [q/Esc] quit ",
        Style::default().fg(Color::Black).bg(Color::Gray),
    )]));
    frame.render_widget(bar, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{AgentRow, MessageRow, TaskRow, TraceDetail};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            agents: vec![AgentRow {
                id: "claude-1".into(),
                tool: "claude".into(),
                status: "busy".into(),
                cost_class: 3,
                current_task: Some("t-1".into()),
            }],
            tasks: vec![
                TaskRow {
                    id: "t-1".into(),
                    kind: "implement".into(),
                    state: "working".into(),
                    assignee: "claude-1".into(),
                    parent: Some("root-1".into()),
                    deps: vec![],
                },
                TaskRow {
                    id: "t-2".into(),
                    kind: "review".into(),
                    state: "open".into(),
                    assignee: "-".into(),
                    parent: Some("root-1".into()),
                    deps: vec!["t-1".into()],
                },
            ],
            messages: vec![MessageRow {
                from: "claude-1".into(),
                to: "codex-1".into(),
                kind: "result".into(),
                summary: "implemented feature".into(),
                origin: None,
                delivered: true,
            }],
            trace: TraceDetail {
                task_id: "t-1".into(),
                text: "10:00 task_created\n10:01 message_enqueue".into(),
                metrics_line: "messages=1 avg_summary_len=18.0 max=18 injections=0 turns=1".into(),
                event_count: 2,
            },
            daemon_down: false,
        }
    }

    /// Render a snapshot to a string via `TestBackend` (the same path `--once`
    /// uses). This is the non-interactive smoke test for the screen.
    fn render_to_string(snapshot: &Snapshot, focus: Focus, selected: usize) -> String {
        let backend = TestBackend::new(200, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render(f, snapshot, focus, selected))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect::<String>()
    }

    #[test]
    fn renders_non_empty_for_sample_snapshot() {
        let out = render_to_string(&sample_snapshot(), Focus::Tasks, 0);
        assert!(out.contains("Agents"));
        assert!(out.contains("Tasks"));
        assert!(out.contains("Messages"));
        assert!(out.contains("Trace"));
        // Real data made it onto the screen.
        assert!(out.contains("claude-1"));
        assert!(out.contains("t-1"));
        assert!(out.contains("implemented feature"));
        assert!(out.contains("task_created"));
        assert!(out.contains("METRICS"));
        // §F4.2 projection: agent current task + task dependency edges visible.
        assert!(out.contains("@t-1"), "agent current_task shown");
        assert!(out.contains("deps[t-1]"), "task dependency edge shown");
    }

    #[test]
    fn renders_daemon_down_banner() {
        let out = render_to_string(&Snapshot::daemon_down(), Focus::Tasks, 0);
        assert!(out.contains("daemon not running (divan up)"));
    }

    #[test]
    fn focus_cycles_through_all_panels() {
        assert_eq!(Focus::Tasks.next(), Focus::Agents);
        assert_eq!(Focus::Agents.next(), Focus::Messages);
        assert_eq!(Focus::Messages.next(), Focus::Trace);
        assert_eq!(Focus::Trace.next(), Focus::Tasks);
    }

    #[test]
    fn render_does_not_panic_on_empty_snapshot() {
        let out = render_to_string(&Snapshot::default(), Focus::Agents, 99);
        assert!(out.contains("no tasks") || out.contains("(no"));
    }
}
