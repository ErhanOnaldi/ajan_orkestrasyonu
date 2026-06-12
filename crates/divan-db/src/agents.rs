//! Agent registry repository (impl plan §5.1, §F1.2).

use crate::store::{Db, DbResult};
use divan_core::{AgentCard, AgentId, AgentStatus, AgentTool, Capability, DeliveryKind};
use rusqlite::{params, Row};

/// Agent registry repository surface (impl plan §F1.2).
pub trait AgentStore {
    /// Insert or idempotently update an agent card (impl plan §5.1 test).
    fn upsert_agent(&self, card: &AgentCard) -> DbResult<()>;
    fn get_agent(&self, id: &AgentId) -> DbResult<Option<AgentCard>>;
    fn list_agents(&self) -> DbResult<Vec<AgentCard>>;
    /// Update status (+ optional bound session) — heartbeat / reconciliation.
    fn set_agent_status(
        &self,
        id: &AgentId,
        status: AgentStatus,
        session_id: Option<&str>,
    ) -> DbResult<()>;
}

fn row_to_card(row: &Row<'_>) -> rusqlite::Result<AgentCard> {
    let tool: String = row.get("tool")?;
    let caps: String = row.get("capabilities")?;
    let skills: Option<String> = row.get("skills")?;
    let delivery: String = row.get("delivery")?;
    let status: String = row.get("status")?;
    let multi_turn: i64 = row.get("multi_turn")?;

    let parse = |json: &str, col: &str| -> rusqlite::Result<serde_json::Value> {
        serde_json::from_str(json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{col}: {e}"),
                )),
            )
        })
    };
    let caps_v: Vec<Capability> =
        serde_json::from_value(parse(&caps, "capabilities")?).unwrap_or_default();
    let delivery_v: Vec<DeliveryKind> =
        serde_json::from_value(parse(&delivery, "delivery")?).unwrap_or_default();
    let skills_v: Vec<String> = match skills {
        Some(s) => serde_json::from_value(parse(&s, "skills")?).unwrap_or_default(),
        None => vec![],
    };
    let tool_v: AgentTool =
        serde_json::from_value(serde_json::Value::String(tool)).unwrap_or(AgentTool::Claude);
    let status_v: AgentStatus =
        serde_json::from_value(serde_json::Value::String(status)).unwrap_or(AgentStatus::Offline);

    Ok(AgentCard {
        id: AgentId::new(row.get::<_, String>("id")?),
        tool: tool_v,
        display_name: row.get("display_name")?,
        capabilities: caps_v,
        cost_class: row.get::<_, i64>("cost_class")? as u8,
        skills: skills_v,
        delivery: delivery_v,
        multi_turn: multi_turn != 0,
        status: status_v,
        session_id: row.get("session_id")?,
        registered_at: row.get("registered_at")?,
    })
}

impl AgentStore for Db {
    fn upsert_agent(&self, card: &AgentCard) -> DbResult<()> {
        let conn = self.lock()?;
        let caps = serde_json::to_string(&card.capabilities)?;
        let skills = serde_json::to_string(&card.skills)?;
        let delivery = serde_json::to_string(&card.delivery)?;
        let tool = card.tool.as_str();
        let status = serde_json::to_value(card.status)?
            .as_str()
            .unwrap_or("offline")
            .to_string();
        conn.execute(
            "INSERT INTO agents
               (id, tool, display_name, capabilities, cost_class, skills, delivery,
                multi_turn, status, session_id, registered_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(id) DO UPDATE SET
                tool=excluded.tool, display_name=excluded.display_name,
                capabilities=excluded.capabilities, cost_class=excluded.cost_class,
                skills=excluded.skills, delivery=excluded.delivery,
                multi_turn=excluded.multi_turn, status=excluded.status,
                session_id=excluded.session_id",
            params![
                card.id.as_str(),
                tool,
                card.display_name,
                caps,
                card.cost_class as i64,
                skills,
                delivery,
                card.multi_turn as i64,
                status,
                card.session_id,
                card.registered_at,
            ],
        )?;
        Ok(())
    }

    fn get_agent(&self, id: &AgentId) -> DbResult<Option<AgentCard>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM agents WHERE id = ?1")?;
        let mut rows = stmt.query_map([id.as_str()], row_to_card)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    fn list_agents(&self) -> DbResult<Vec<AgentCard>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM agents ORDER BY id")?;
        let rows = stmt.query_map([], row_to_card)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn set_agent_status(
        &self,
        id: &AgentId,
        status: AgentStatus,
        session_id: Option<&str>,
    ) -> DbResult<()> {
        let conn = self.lock()?;
        let status_s = serde_json::to_value(status)?
            .as_str()
            .unwrap_or("offline")
            .to_string();
        conn.execute(
            "UPDATE agents SET status = ?2, session_id = COALESCE(?3, session_id) WHERE id = ?1",
            params![id.as_str(), status_s, session_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> AgentCard {
        AgentCard {
            id: AgentId::new(id),
            tool: AgentTool::Claude,
            display_name: Some("Claude".into()),
            capabilities: vec![Capability::Read, Capability::Write],
            cost_class: 5,
            skills: vec!["implement".into()],
            delivery: vec![DeliveryKind::Hook, DeliveryKind::Mcp],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 100,
        }
    }

    #[test]
    fn upsert_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("claude-1")).unwrap();
        let mut c2 = card("claude-1");
        c2.cost_class = 3; // re-register with a change
        db.upsert_agent(&c2).unwrap();
        assert_eq!(db.list_agents().unwrap().len(), 1, "no duplicate row");
        assert_eq!(
            db.get_agent(&AgentId::new("claude-1"))
                .unwrap()
                .unwrap()
                .cost_class,
            3
        );
    }

    #[test]
    fn roundtrip_preserves_json_columns() {
        let db = Db::open_in_memory().unwrap();
        let c = card("claude-1");
        db.upsert_agent(&c).unwrap();
        let got = db.get_agent(&AgentId::new("claude-1")).unwrap().unwrap();
        assert_eq!(got, c);
    }

    #[test]
    fn set_status_updates_session() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("claude-1")).unwrap();
        db.set_agent_status(
            &AgentId::new("claude-1"),
            AgentStatus::Offline,
            Some("sess-9"),
        )
        .unwrap();
        let got = db.get_agent(&AgentId::new("claude-1")).unwrap().unwrap();
        assert_eq!(got.status, AgentStatus::Offline);
        assert_eq!(got.session_id.as_deref(), Some("sess-9"));
    }
}
