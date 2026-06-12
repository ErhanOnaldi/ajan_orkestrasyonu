//! Message repository (impl plan §5.3, §F1.2).
//!
//! Faz 1 needs pointer-message persistence with summary validation (K4); the
//! full subscription fan-out, turn batching, and delivery receipts are Faz 2
//! (impl plan §5.3, §F2.x). The schema already carries `origin_message_id` so
//! the Faz 2 fan-out (P0.2 Option A) needs no migration.

use crate::store::{Db, DbError, DbResult};
use divan_core::{validate_summary, Message, MessageId};
use rusqlite::{params, Row};

/// Message repository surface (impl plan §F1.2).
pub trait MessageStore {
    /// Insert a message after validating the summary contract (K4).
    fn insert_message(&self, msg: &Message) -> DbResult<()>;
    fn get_message(&self, id: &MessageId) -> DbResult<Option<Message>>;
    fn list_messages(&self) -> DbResult<Vec<Message>>;
}

fn row_to_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    let kind: String = row.get("kind")?;
    let payload: Option<String> = row.get("payload")?;
    Ok(Message {
        id: MessageId::new(row.get::<_, String>("id")?),
        origin_message_id: row
            .get::<_, Option<String>>("origin_message_id")?
            .map(MessageId::new),
        from_agent: divan_core::AgentId::new(row.get::<_, String>("from_agent")?),
        to_agent: row
            .get::<_, Option<String>>("to_agent")?
            .map(divan_core::AgentId::new),
        kind: serde_json::from_value(serde_json::Value::String(kind))
            .unwrap_or(divan_core::MessageKind::Status),
        summary: row.get("summary")?,
        payload: payload.and_then(|p| serde_json::from_str(&p).ok()),
        artifact_ref: row
            .get::<_, Option<String>>("artifact_ref")?
            .map(divan_core::ArtifactRef::new),
        task_id: row
            .get::<_, Option<String>>("task_id")?
            .map(divan_core::TaskId::new),
        trace_id: row
            .get::<_, Option<String>>("trace_id")?
            .map(divan_core::TraceId::new),
        created_at: row.get("created_at")?,
        delivered_at: row.get("delivered_at")?,
    })
}

impl MessageStore for Db {
    fn insert_message(&self, msg: &Message) -> DbResult<()> {
        // K4 contract enforced at the API layer (DB CHECK is the backstop).
        validate_summary(&msg.summary).map_err(DbError::Domain)?;
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO messages
               (id, origin_message_id, from_agent, to_agent, kind, summary, payload,
                artifact_ref, task_id, trace_id, created_at, delivered_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                msg.id.as_str(),
                msg.origin_message_id.as_ref().map(|m| m.as_str()),
                msg.from_agent.as_str(),
                msg.to_agent.as_ref().map(|a| a.as_str()),
                msg.kind.as_str(),
                msg.summary,
                msg.payload.as_ref().map(|p| p.to_string()),
                msg.artifact_ref.as_ref().map(|a| a.as_str()),
                msg.task_id.as_ref().map(|t| t.as_str()),
                msg.trace_id.as_ref().map(|t| t.as_str()),
                msg.created_at,
                msg.delivered_at,
            ],
        )?;
        Ok(())
    }

    fn get_message(&self, id: &MessageId) -> DbResult<Option<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM messages WHERE id = ?1")?;
        let mut rows = stmt.query_map([id.as_str()], row_to_message)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn list_messages(&self) -> DbResult<Vec<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM messages ORDER BY created_at, id")?;
        let rows = stmt.query_map([], row_to_message)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{AgentId, MessageKind};

    fn msg(id: &str, summary: &str) -> Message {
        Message {
            id: MessageId::new(id),
            origin_message_id: None,
            from_agent: AgentId::new("claude-1"),
            to_agent: Some(AgentId::new("codex-1")),
            kind: MessageKind::Handoff,
            summary: summary.to_string(),
            payload: None,
            artifact_ref: Some(divan_core::ArtifactRef::new("abc123")),
            task_id: None,
            trace_id: None,
            created_at: 1,
            delivered_at: None,
        }
    }

    #[test]
    fn insert_and_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let m = msg("m1", "handoff ready");
        db.insert_message(&m).unwrap();
        assert_eq!(db.get_message(&MessageId::new("m1")).unwrap().unwrap(), m);
    }

    #[test]
    fn oversize_summary_rejected_at_api() {
        let db = Db::open_in_memory().unwrap();
        let m = msg("m1", &"x".repeat(401));
        let err = db.insert_message(&m).unwrap_err();
        assert!(matches!(err, DbError::Domain(_)));
    }
}
