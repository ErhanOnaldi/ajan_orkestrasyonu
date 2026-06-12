//! Message repository (impl plan §5.3, §F1.2).
//!
//! Faz 1 needs pointer-message persistence with summary validation (K4); the
//! full subscription fan-out, turn batching, and delivery receipts are Faz 2
//! (impl plan §5.3, §F2.x). The schema already carries `origin_message_id` so
//! the Faz 2 fan-out (P0.2 Option A) needs no migration.

use crate::store::{Db, DbError, DbResult};
use divan_core::{validate_summary, Message, MessageId};
use rusqlite::{params, Row};

/// Message repository surface (impl plan §F1.2, §F2.4/§F2.5).
pub trait MessageStore {
    /// Insert a message after validating the summary contract (K4).
    fn insert_message(&self, msg: &Message) -> DbResult<()>;
    fn get_message(&self, id: &MessageId) -> DbResult<Option<Message>>;
    fn list_messages(&self) -> DbResult<Vec<Message>>;

    /// Fan a broadcast out to `targets` (P0.2 Option A, impl plan §F2.4):
    /// store the `source` row once with `to_agent = NULL` (never queued), then a
    /// per-target copy with `to_agent` set and `origin_message_id = source.id`.
    /// Returns the copy ids. A target equal to the sender is skipped.
    fn fanout(&self, source: &Message, targets: &[AgentId]) -> DbResult<Vec<MessageId>>;

    /// Undelivered messages queued for `agent` (`to_agent = agent AND
    /// delivered_at IS NULL`), oldest first. Source broadcast rows
    /// (`to_agent = NULL`) are never returned (impl plan §F2.5).
    fn pending_for(&self, agent: &AgentId) -> DbResult<Vec<Message>>;

    /// Stamp `delivered_at = ts` on the given target rows after a receipt
    /// (impl plan §F2.5). Returns the number of rows updated.
    fn mark_delivered(&self, ids: &[MessageId], ts: i64) -> DbResult<usize>;
}

use divan_core::AgentId;

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

/// Insert one message row via any `Connection` (or `Transaction`, which derefs
/// to `Connection`). Shared by direct insert and fan-out.
fn insert_row(conn: &rusqlite::Connection, msg: &Message) -> rusqlite::Result<()> {
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

impl MessageStore for Db {
    fn insert_message(&self, msg: &Message) -> DbResult<()> {
        // K4 contract enforced at the API layer (DB CHECK is the backstop).
        validate_summary(&msg.summary).map_err(DbError::Domain)?;
        let conn = self.lock()?;
        insert_row(&conn, msg)?;
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

    fn fanout(&self, source: &Message, targets: &[AgentId]) -> DbResult<Vec<MessageId>> {
        validate_summary(&source.summary).map_err(DbError::Domain)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        // Source broadcast row: stored once, to_agent = NULL, never queued.
        insert_row(&tx, source)?;
        let mut copy_ids = Vec::new();
        for target in targets {
            if target == &source.from_agent {
                continue; // never deliver a broadcast back to its sender (K6)
            }
            let copy_id = MessageId::new(format!("{}#{}", source.id, target));
            let copy = Message {
                id: copy_id.clone(),
                origin_message_id: Some(source.id.clone()),
                to_agent: Some(target.clone()),
                delivered_at: None,
                ..source.clone()
            };
            insert_row(&tx, &copy)?;
            copy_ids.push(copy_id);
        }
        tx.commit()?;
        Ok(copy_ids)
    }

    fn pending_for(&self, agent: &AgentId) -> DbResult<Vec<Message>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT * FROM messages
             WHERE to_agent = ?1 AND delivered_at IS NULL
             ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([agent.as_str()], row_to_message)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn mark_delivered(&self, ids: &[MessageId], ts: i64) -> DbResult<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut n = 0;
        for id in ids {
            // Only stamp target rows that are still pending (idempotent receipt).
            n += tx.execute(
                "UPDATE messages SET delivered_at = ?2
                 WHERE id = ?1 AND delivered_at IS NULL AND to_agent IS NOT NULL",
                params![id.as_str(), ts],
            )?;
        }
        tx.commit()?;
        Ok(n)
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

    fn broadcast(id: &str, from: &str, summary: &str) -> Message {
        Message {
            id: MessageId::new(id),
            origin_message_id: None,
            from_agent: AgentId::new(from),
            to_agent: None, // source broadcast row
            kind: MessageKind::Question,
            summary: summary.to_string(),
            payload: None,
            artifact_ref: None,
            task_id: Some(divan_core::TaskId::new("impl-1")),
            trace_id: Some(divan_core::TraceId::new("tr-1")),
            created_at: 10,
            delivered_at: None,
        }
    }

    #[test]
    fn fanout_creates_source_and_linked_target_copies() {
        let db = Db::open_in_memory().unwrap();
        let src = broadcast("b1", "claude-1", "anyone seen the spec?");
        let copies = db
            .fanout(&src, &[AgentId::new("codex-1"), AgentId::new("agy-1")])
            .unwrap();
        assert_eq!(copies.len(), 2);
        // Source row stored, to_agent NULL, never in any queue.
        let src_row = db.get_message(&MessageId::new("b1")).unwrap().unwrap();
        assert_eq!(src_row.to_agent, None);
        // Each copy links back via origin_message_id and has to_agent set.
        for (copy_id, target) in copies.iter().zip(["codex-1", "agy-1"]) {
            let c = db.get_message(copy_id).unwrap().unwrap();
            assert_eq!(c.origin_message_id, Some(MessageId::new("b1")));
            assert_eq!(c.to_agent, Some(AgentId::new(target)));
            assert_eq!(c.delivered_at, None);
        }
        // pending_for returns the target copy, never the source row.
        let pend = db.pending_for(&AgentId::new("codex-1")).unwrap();
        assert_eq!(pend.len(), 1);
        assert_eq!(pend[0].origin_message_id, Some(MessageId::new("b1")));
    }

    #[test]
    fn fanout_skips_sender() {
        let db = Db::open_in_memory().unwrap();
        let src = broadcast("b1", "claude-1", "hi");
        let copies = db
            .fanout(&src, &[AgentId::new("claude-1"), AgentId::new("codex-1")])
            .unwrap();
        assert_eq!(copies.len(), 1, "sender excluded");
        assert!(db
            .pending_for(&AgentId::new("claude-1"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn mark_delivered_stamps_and_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let src = broadcast("b1", "claude-1", "q");
        let copies = db.fanout(&src, &[AgentId::new("codex-1")]).unwrap();
        let n = db.mark_delivered(&copies, 99).unwrap();
        assert_eq!(n, 1);
        // Now delivered => not pending; second receipt is a no-op.
        assert!(db.pending_for(&AgentId::new("codex-1")).unwrap().is_empty());
        assert_eq!(db.mark_delivered(&copies, 100).unwrap(), 0);
        let c = db.get_message(&copies[0]).unwrap().unwrap();
        assert_eq!(c.delivered_at, Some(99));
    }

    #[test]
    fn source_row_is_never_delivered() {
        let db = Db::open_in_memory().unwrap();
        let src = broadcast("b1", "claude-1", "q");
        db.fanout(&src, &[AgentId::new("codex-1")]).unwrap();
        // Attempting to mark the source row delivered does nothing (to_agent NULL).
        assert_eq!(db.mark_delivered(&[MessageId::new("b1")], 5).unwrap(), 0);
    }
}
