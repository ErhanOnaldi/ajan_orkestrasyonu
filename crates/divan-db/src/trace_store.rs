//! Trace event repository (impl plan §5.7, §F1.2). Append-only, cheap writes.

use crate::store::{Db, DbResult};
use divan_core::{SpanId, TraceEvent, TraceEventKind, TraceId};
use rusqlite::{params, Row};

/// Trace repository surface (impl plan §F1.2).
pub trait TraceStore {
    /// Append one trace event (K10). Most behaviors emit via their own tx; this
    /// is for standalone events (spawn, router_decision, etc.).
    fn append_event(&self, event: &TraceEvent) -> DbResult<()>;
    /// All events for a trace, time-ordered (for `divan log` / `divan trace`).
    fn timeline(&self, trace_id: &TraceId) -> DbResult<Vec<TraceEvent>>;
    /// The most recent `limit` events across all traces, oldest-first
    /// (for an unfiltered `divan log`).
    fn recent_events(&self, limit: usize) -> DbResult<Vec<TraceEvent>>;
}

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<TraceEvent> {
    let event_s: String = row.get("event")?;
    let event: TraceEventKind =
        serde_json::from_value(serde_json::Value::String(event_s)).unwrap_or(TraceEventKind::Error);
    let data: Option<String> = row.get("data")?;
    let data_v = data.and_then(|d| serde_json::from_str(&d).ok());
    Ok(TraceEvent {
        trace_id: TraceId::new(row.get::<_, String>("trace_id")?),
        span_id: row.get::<_, Option<String>>("span_id")?.map(SpanId::new),
        parent_span: row
            .get::<_, Option<String>>("parent_span")?
            .map(SpanId::new),
        agent_id: row
            .get::<_, Option<String>>("agent_id")?
            .map(divan_core::AgentId::new),
        event,
        data: data_v,
        ts: row.get("ts")?,
    })
}

impl TraceStore for Db {
    fn append_event(&self, event: &TraceEvent) -> DbResult<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO trace_events (trace_id, span_id, parent_span, agent_id, event, data, ts)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                event.trace_id.as_str(),
                event.span_id.as_ref().map(|s| s.as_str()),
                event.parent_span.as_ref().map(|s| s.as_str()),
                event.agent_id.as_ref().map(|a| a.as_str()),
                event.event.as_str(),
                event.data.as_ref().map(|d| d.to_string()),
                event.ts,
            ],
        )?;
        Ok(())
    }

    fn timeline(&self, trace_id: &TraceId) -> DbResult<Vec<TraceEvent>> {
        let conn = self.lock()?;
        let mut stmt =
            conn.prepare("SELECT * FROM trace_events WHERE trace_id = ?1 ORDER BY ts, id")?;
        let rows = stmt.query_map([trace_id.as_str()], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn recent_events(&self, limit: usize) -> DbResult<Vec<TraceEvent>> {
        let conn = self.lock()?;
        // Take the newest `limit` by id, then return oldest-first for display.
        let mut stmt = conn.prepare("SELECT * FROM trace_events ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], row_to_event)?;
        let mut v = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        v.reverse();
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_back_with_data() {
        let db = Db::open_in_memory().unwrap();
        let ev = TraceEvent::new(TraceId::new("tr"), TraceEventKind::Spawn, 5)
            .with_agent(divan_core::AgentId::new("claude-1"))
            .with_data(serde_json::json!({"pid": 123}));
        db.append_event(&ev).unwrap();
        let tl = db.timeline(&TraceId::new("tr")).unwrap();
        assert_eq!(tl.len(), 1);
        assert_eq!(tl[0].event, TraceEventKind::Spawn);
        assert_eq!(tl[0].data.as_ref().unwrap()["pid"], 123);
    }

    #[test]
    fn timeline_is_time_ordered() {
        let db = Db::open_in_memory().unwrap();
        for ts in [30, 10, 20] {
            db.append_event(&TraceEvent::new(
                TraceId::new("tr"),
                TraceEventKind::ToolCall,
                ts,
            ))
            .unwrap();
        }
        let tl = db.timeline(&TraceId::new("tr")).unwrap();
        assert_eq!(
            tl.iter().map(|e| e.ts).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }
}
