//! The data layer for the TUI (impl plan §F4.2). A [`Snapshot`] is a flat,
//! testable view of the daemon built from the `status`, `messages`, and `trace`
//! RPC responses. All parsing here is pure (JSON -> structs) so it can be
//! unit-tested without a daemon or a TTY.

use serde_json::Value;

/// One agent row (from `status` → `agents[]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentRow {
    pub id: String,
    pub tool: String,
    pub status: String,
    pub cost_class: u64,
    /// The agent's currently claimed/working task, if any (§F4.2 panel).
    pub current_task: Option<String>,
}

/// One task row (from `status` → `tasks[]`). This is the navigable list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskRow {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub assignee: String,
    /// Parent task id (the task-tree edge), if any (§F4.2).
    pub parent: Option<String>,
    /// Dependency edges: task ids this task is blocked by (§F4.2 deps).
    pub deps: Vec<String>,
}

/// One message row (from `messages` → `messages[]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessageRow {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub summary: String,
    /// Source broadcast id for a fan-out copy (the message-edge, F4.1), if any.
    pub origin: Option<String>,
    pub delivered: bool,
}

/// The trace/detail panel content for the currently-selected task
/// (from `trace {id}` → `{text, metrics, event_count}`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TraceDetail {
    /// Which task id this detail was fetched for (empty if none selected).
    pub task_id: String,
    pub text: String,
    pub metrics_line: String,
    pub event_count: u64,
}

/// A flat view of the whole hub for one render pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub agents: Vec<AgentRow>,
    pub tasks: Vec<TaskRow>,
    pub messages: Vec<MessageRow>,
    pub trace: TraceDetail,
    /// Set when the daemon is unreachable; the UI shows a banner instead of data.
    pub daemon_down: bool,
}

impl Snapshot {
    /// The "daemon not running" state — shown in `--once` and in the live loop
    /// when RPC calls fail, instead of crashing.
    pub fn daemon_down() -> Self {
        Self {
            daemon_down: true,
            ..Default::default()
        }
    }
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Parse the `agents[]` array of a `status` result into rows.
pub fn parse_agents(status_result: &Value) -> Vec<AgentRow> {
    status_result
        .get("agents")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| AgentRow {
                    id: s(a, "id"),
                    tool: s(a, "tool"),
                    status: s(a, "status"),
                    cost_class: a.get("cost_class").and_then(|c| c.as_u64()).unwrap_or(0),
                    current_task: a
                        .get("current_task")
                        .and_then(|x| x.as_str())
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `tasks[]` array of a `status` result into rows.
pub fn parse_tasks(status_result: &Value) -> Vec<TaskRow> {
    status_result
        .get("tasks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| TaskRow {
                    id: s(t, "id"),
                    kind: s(t, "kind"),
                    state: s(t, "state"),
                    assignee: t
                        .get("assignee")
                        .and_then(|x| x.as_str())
                        .unwrap_or("-")
                        .to_string(),
                    parent: t.get("parent").and_then(|x| x.as_str()).map(str::to_string),
                    deps: t
                        .get("deps")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|d| d.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `messages[]` array of a `messages` result into rows.
pub fn parse_messages(messages_result: &Value) -> Vec<MessageRow> {
    messages_result
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| MessageRow {
                    from: s(m, "from"),
                    to: m
                        .get("to")
                        .and_then(|x| x.as_str())
                        .unwrap_or("(broadcast)")
                        .to_string(),
                    kind: s(m, "kind"),
                    summary: s(m, "summary"),
                    origin: m.get("origin").and_then(|x| x.as_str()).map(str::to_string),
                    delivered: m
                        .get("delivered")
                        .and_then(|d| d.as_bool())
                        .unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Format the `metrics` object of a `trace` result into a one-line summary
/// (mirrors the CLI's `divan trace` metrics line; F4.3 proxy measures, no
/// "% savings" claim).
pub fn format_metrics(metrics: &Value) -> String {
    if !metrics.is_object() {
        return String::new();
    }
    let u = |k: &str| metrics.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let f = |k: &str| metrics.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let mut line = format!(
        "messages={} avg_summary_len={:.1} max={} injections={} turns={}",
        u("message_count"),
        f("avg_summary_len"),
        u("max_summary_len"),
        u("injections"),
        u("turns"),
    );
    if let Some(dist) = metrics.get("cost_class_dist").and_then(|v| v.as_object()) {
        let parts: Vec<String> = dist.iter().map(|(k, v)| format!("c{k}={v}")).collect();
        if !parts.is_empty() {
            line.push_str(&format!("  cost_class_dist: {}", parts.join(" ")));
        }
    }
    line
}

/// Build a [`TraceDetail`] from a `trace` result for the given task id.
pub fn parse_trace(task_id: &str, trace_result: &Value) -> TraceDetail {
    TraceDetail {
        task_id: task_id.to_string(),
        text: trace_result
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        metrics_line: trace_result
            .get("metrics")
            .map(format_metrics)
            .unwrap_or_default(),
        event_count: trace_result
            .get("event_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_status() -> Value {
        json!({
            "agents": [
                {"id": "claude-1", "tool": "claude", "status": "busy", "cost_class": 3,
                 "current_task": "t-1"},
                {"id": "codex-1", "tool": "codex", "status": "idle", "cost_class": 1,
                 "current_task": null}
            ],
            "tasks": [
                {"id": "t-1", "kind": "implement", "state": "working", "assignee": "claude-1",
                 "parent": "root-1", "deps": []},
                {"id": "t-2", "kind": "review", "state": "open", "assignee": null,
                 "parent": "root-1", "deps": ["t-1"]}
            ]
        })
    }

    #[test]
    fn parses_agents() {
        let agents = parse_agents(&sample_status());
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].id, "claude-1");
        assert_eq!(agents[0].tool, "claude");
        assert_eq!(agents[0].status, "busy");
        assert_eq!(agents[0].cost_class, 3);
        assert_eq!(agents[0].current_task.as_deref(), Some("t-1"));
        assert_eq!(agents[1].cost_class, 1);
        assert_eq!(agents[1].current_task, None);
    }

    #[test]
    fn parses_tasks_with_null_assignee() {
        let tasks = parse_tasks(&sample_status());
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "t-1");
        assert_eq!(tasks[0].state, "working");
        assert_eq!(tasks[0].assignee, "claude-1");
        assert_eq!(tasks[0].parent.as_deref(), Some("root-1"));
        assert!(tasks[0].deps.is_empty());
        // Null assignee falls back to "-"; review depends on the implement task.
        assert_eq!(tasks[1].assignee, "-");
        assert_eq!(tasks[1].deps, vec!["t-1".to_string()]);
    }

    #[test]
    fn parses_messages_with_broadcast_fallback() {
        let result = json!({
            "messages": [
                {"from": "claude-1", "to": "codex-1", "kind": "result", "summary": "done", "delivered": true, "origin": "b-1"},
                {"from": "codex-1", "to": null, "kind": "note", "summary": "fyi", "delivered": false}
            ]
        });
        let msgs = parse_messages(&result);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].from, "claude-1");
        assert_eq!(msgs[0].to, "codex-1");
        assert!(msgs[0].delivered);
        assert_eq!(msgs[0].origin.as_deref(), Some("b-1")); // fan-out edge
        assert_eq!(msgs[1].to, "(broadcast)");
        assert_eq!(msgs[1].origin, None);
        assert!(!msgs[1].delivered);
    }

    #[test]
    fn parse_trace_extracts_text_and_metrics() {
        let result = json!({
            "text": "10:00 task_created\n10:01 message_enqueue\n",
            "metrics": {
                "message_count": 4,
                "avg_summary_len": 12.5,
                "max_summary_len": 40,
                "injections": 2,
                "turns": 3,
                "cost_class_dist": {"1": 2, "3": 1}
            },
            "event_count": 2
        });
        let d = parse_trace("t-1", &result);
        assert_eq!(d.task_id, "t-1");
        assert!(d.text.contains("task_created"));
        assert_eq!(d.event_count, 2);
        assert!(d.metrics_line.contains("messages=4"));
        assert!(d.metrics_line.contains("avg_summary_len=12.5"));
        assert!(d.metrics_line.contains("cost_class_dist"));
    }

    #[test]
    fn parsers_tolerate_missing_fields() {
        // Empty/garbage results must not panic and yield empty rows (F4.1: a
        // missing/empty timeline never breaks the screen).
        assert!(parse_agents(&json!({})).is_empty());
        assert!(parse_tasks(&json!({})).is_empty());
        assert!(parse_messages(&json!(null)).is_empty());
        let d = parse_trace("", &json!({}));
        assert_eq!(d.text, "");
        assert_eq!(d.event_count, 0);
        assert_eq!(d.metrics_line, "");
    }
}
