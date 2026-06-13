//! `divan-trace` — trace timeline query/render models (K10, spec §5.3).
//!
//! Faz 1 needs only the query DTOs used to render `divan log`; the full TUI
//! timeline and metrics aggregation are Faz 4 (impl plan §5.7, §F4.1). Persisted
//! trace rows live in `divan-db`'s `TraceStore`; this crate shapes them for
//! presentation.

use divan_core::trace::{TraceEvent, TraceEventKind};
use divan_core::{CostClass, Message};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A flat, time-ordered view of one trace's events for `divan log`/`divan trace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    pub trace_id: String,
    pub events: Vec<TraceEvent>,
}

impl Timeline {
    pub fn new(trace_id: impl Into<String>, mut events: Vec<TraceEvent>) -> Self {
        events.sort_by_key(|e| e.ts);
        Self {
            trace_id: trace_id.into(),
            events,
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Render a robust text timeline (`divan trace <id>`, F4.1). One line per
    /// event: `<ts>  <event>  [agent]  <key fields>`. Tolerant of missing/unknown
    /// fields — a sparse event never breaks the render (F4.1 acceptance).
    pub fn render_text(&self) -> String {
        let mut out = format!("trace {} — {} event(s)\n", self.trace_id, self.events.len());
        for e in &self.events {
            let agent = e.agent_id.as_ref().map(|a| a.as_str()).unwrap_or("-");
            let detail = e.data.as_ref().map(summarize_data).unwrap_or_default();
            out.push_str(&format!(
                "  {:>16}  {:<22} {:<10} {}\n",
                e.ts,
                e.event.as_str(),
                agent,
                detail
            ));
        }
        out
    }

    /// Group event indices by span id (`None` span → the "-" bucket). F4.1 span
    /// grouping for the TUI's per-session lanes.
    pub fn spans(&self) -> BTreeMap<String, Vec<usize>> {
        let mut by_span: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in self.events.iter().enumerate() {
            let key = e
                .span_id
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into());
            by_span.entry(key).or_default().push(i);
        }
        by_span
    }
}

/// Pull the most useful fields out of an event's JSON `data` for a one-line view.
fn summarize_data(data: &serde_json::Value) -> String {
    let pick = |k: &str| data.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    // Prefer the fields that carry meaning across event kinds.
    for key in [
        "reason", "to", "name", "kind", "ref", "branch", "message", "path",
    ] {
        if let Some(v) = pick(key) {
            return format!("{key}={v}");
        }
    }
    // Fall back to a compact JSON of the object (bounded).
    let s = data.to_string();
    if s == "null" {
        String::new()
    } else {
        s.chars().take(80).collect()
    }
}

/// Token/cost metrics for a trace (K10/§5.3, F4.3). Proxy measures only — NO
/// "percent savings" claim (spec §9, risk table).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub message_count: usize,
    /// Average message summary length in characters (the K4 ≤400 discipline).
    pub avg_summary_len: f64,
    pub max_summary_len: usize,
    /// Number of delivered injections (msg_delivered events).
    pub injections: usize,
    /// Number of turn boundaries observed (session_event/turn_end).
    pub turns: usize,
    /// Task count per assignee cost_class (1 cheap .. 5 expensive).
    pub cost_class_dist: BTreeMap<u8, usize>,
}

/// Compute trace metrics from its messages, events, and the assignee cost
/// classes of its tasks. Pure + deterministic (K2).
pub fn compute_metrics(
    messages: &[Message],
    events: &[TraceEvent],
    task_cost_classes: &[CostClass],
) -> Metrics {
    let message_count = messages.len();
    let total_len: usize = messages.iter().map(|m| m.summary.chars().count()).sum();
    let max_summary_len = messages
        .iter()
        .map(|m| m.summary.chars().count())
        .max()
        .unwrap_or(0);
    let avg_summary_len = if message_count > 0 {
        total_len as f64 / message_count as f64
    } else {
        0.0
    };
    let injections = events
        .iter()
        .filter(|e| e.event == TraceEventKind::MsgDelivered)
        .count();
    let turns = events
        .iter()
        .filter(|e| {
            e.event == TraceEventKind::SessionEvent
                && e.data
                    .as_ref()
                    .and_then(|d| d.get("event"))
                    .and_then(|v| v.as_str())
                    == Some("turn_end")
        })
        .count();
    let mut cost_class_dist = BTreeMap::new();
    for &cc in task_cost_classes {
        *cost_class_dist.entry(cc).or_insert(0) += 1;
    }
    Metrics {
        message_count,
        avg_summary_len,
        max_summary_len,
        injections,
        turns,
        cost_class_dist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::trace::TraceEventKind;
    use divan_core::TraceId;

    #[test]
    fn timeline_sorts_by_ts() {
        let evs = vec![
            TraceEvent::new(TraceId::new("t"), TraceEventKind::TaskTransition, 30),
            TraceEvent::new(TraceId::new("t"), TraceEventKind::TaskCreated, 10),
        ];
        let tl = Timeline::new("t", evs);
        assert_eq!(tl.events[0].ts, 10);
        assert_eq!(tl.events[1].ts, 30);
    }

    #[test]
    fn render_text_tolerates_missing_data() {
        // An event with no data + one with data: neither breaks the render.
        let evs = vec![
            TraceEvent::new(TraceId::new("t"), TraceEventKind::Spawn, 10),
            TraceEvent::new(TraceId::new("t"), TraceEventKind::TaskTransition, 20)
                .with_data(serde_json::json!({"reason": "completed", "to": "done"})),
        ];
        let text = Timeline::new("t", evs).render_text();
        assert!(text.contains("trace t — 2 event(s)"));
        assert!(text.contains("spawn"));
        assert!(text.contains("reason=completed"));
    }

    #[test]
    fn metrics_avg_summary_and_distribution() {
        use divan_core::{AgentId, Message, MessageKind};
        let msg = |s: &str| Message {
            id: divan_core::MessageId::new("m"),
            origin_message_id: None,
            from_agent: AgentId::new("a"),
            to_agent: Some(AgentId::new("b")),
            kind: MessageKind::Status,
            summary: s.into(),
            payload: None,
            artifact_ref: None,
            task_id: None,
            trace_id: None,
            created_at: 0,
            delivered_at: None,
        };
        let messages = vec![msg("abcd"), msg("ab")]; // lens 4 and 2 -> avg 3
        let events = vec![
            TraceEvent::new(TraceId::new("t"), TraceEventKind::MsgDelivered, 1),
            TraceEvent::new(TraceId::new("t"), TraceEventKind::SessionEvent, 2)
                .with_data(serde_json::json!({"event": "turn_end"})),
        ];
        let m = compute_metrics(&messages, &events, &[5, 4, 5]);
        assert_eq!(m.message_count, 2);
        assert_eq!(m.avg_summary_len, 3.0);
        assert_eq!(m.max_summary_len, 4);
        assert_eq!(m.injections, 1);
        assert_eq!(m.turns, 1);
        assert_eq!(m.cost_class_dist.get(&5), Some(&2));
        assert_eq!(m.cost_class_dist.get(&4), Some(&1));
    }

    #[test]
    fn metrics_empty_is_zero_not_nan() {
        let m = compute_metrics(&[], &[], &[]);
        assert_eq!(m.avg_summary_len, 0.0);
        assert_eq!(m.message_count, 0);
    }
}
