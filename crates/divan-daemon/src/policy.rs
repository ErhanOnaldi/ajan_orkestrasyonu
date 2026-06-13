//! Policy Engine (K8, spec §5.1, impl plan §5.4, §F2.3/§F3.1).
//!
//! Two layers:
//! 1. **MCP tool chokepoint** ([`check`]) — every MCP tool handler routes through
//!    it (F2.3). Gates broadcast (in the bus), `delegate`, and `read` (subscribe).
//! 2. **Execution-action matrix** ([`check_write`], [`check_kill`],
//!    [`check_spawn`]) — the §5.4 actions enforced at execution time (F3.1):
//!    write only inside the assigned worktree (path-canonicalized), kill only
//!    your own session, spawn only within your cost-class limit.
//!
//! Every denial emits a `policy_denied` trace with who/what/why (K8/K10).

use divan_core::{AgentId, Capability, CostClass, TraceEvent, TraceEventKind, TraceId};
use divan_db::{AgentStore, Db, TraceStore};
use std::path::{Component, Path, PathBuf};

/// A policy-relevant action behind an MCP tool (impl plan §5.4 action model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SendDirect,
    Broadcast,
    Delegate,
    Claim,
    Complete,
    Publish,
    GetArtifact,
    Subscribe,
    ListAgents,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::SendDirect => "send_direct",
            Action::Broadcast => "broadcast",
            Action::Delegate => "delegate",
            Action::Claim => "claim",
            Action::Complete => "complete",
            Action::Publish => "publish",
            Action::GetArtifact => "get_artifact",
            Action::Subscribe => "subscribe",
            Action::ListAgents => "list_agents",
        }
    }

    /// Capability required to perform this action in Phase 2. `None` = allowed
    /// for now; the full per-action matrix (spec §5.1) lands in Phase 3.
    pub fn required_cap(self) -> Option<Capability> {
        match self {
            // Broadcast is enforced in the bus (it holds the sender); listed here
            // for completeness so the action model is exhaustive.
            Action::Broadcast => Some(Capability::Broadcast),
            Action::Delegate => Some(Capability::Delegate),
            Action::Subscribe => Some(Capability::Read),
            Action::SendDirect
            | Action::Claim
            | Action::Complete
            | Action::Publish
            | Action::GetArtifact
            | Action::ListAgents => None,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("policy denied: {agent} cannot {action}: {reason}")]
pub struct PolicyDenied {
    pub agent: String,
    pub action: String,
    pub reason: String,
}

/// Build + trace a denial (K8/K10): one place so every `policy_denied` event has
/// the same who/what/why shape.
fn deny(db: &Db, agent: &str, action: &str, reason: String) -> PolicyDenied {
    let _ = db.append_event(
        &TraceEvent::new(
            TraceId::new("policy"),
            TraceEventKind::PolicyDenied,
            divan_core::now_ms(),
        )
        .with_data(serde_json::json!({
            "agent": agent, "action": action, "reason": reason,
        })),
    );
    PolicyDenied {
        agent: agent.to_string(),
        action: action.to_string(),
        reason,
    }
}

fn has_cap(db: &Db, agent: &AgentId, cap: Capability) -> bool {
    db.get_agent(agent)
        .ok()
        .flatten()
        .map(|c| c.has(cap))
        .unwrap_or(false)
}

/// Route an MCP tool action through the chokepoint (F2.3). Actions with no
/// capability requirement pass through; gated ones require the capability.
pub fn check(db: &Db, agent: Option<&AgentId>, action: Action) -> Result<(), PolicyDenied> {
    let Some(cap) = action.required_cap() else {
        return Ok(());
    };
    let ok = matches!(agent, Some(a) if has_cap(db, a, cap));
    if ok {
        return Ok(());
    }
    let who = agent.map(|a| a.to_string()).unwrap_or_default();
    Err(deny(
        db,
        &who,
        action.as_str(),
        format!("lacks {} capability", cap.as_str()),
    ))
}

// ---- Execution-action matrix (spec §5.4 / §5.1, F3.1) ----

/// `write(path)`: requires the `write` capability AND the path must resolve
/// inside the assigned `worktree` (spec §5.1 — "only assigned worktree";
/// path-canonicalized to block `..`/symlink escape). F3.1.
pub fn check_write(
    db: &Db,
    agent: &AgentId,
    path: &Path,
    worktree: &Path,
) -> Result<(), PolicyDenied> {
    if !has_cap(db, agent, Capability::Write) {
        return Err(deny(
            db,
            agent.as_str(),
            "write",
            "lacks write capability".into(),
        ));
    }
    if !path_within(path, worktree) {
        return Err(deny(
            db,
            agent.as_str(),
            "write",
            format!("path {} is outside the assigned worktree", path.display()),
        ));
    }
    Ok(())
}

/// `kill(session)`: requires the `kill` capability AND the caller must own the
/// session (spec §5.1 — "only own spawned sessions"). F3.1.
pub fn check_kill(db: &Db, caller: &AgentId, session_owner: &AgentId) -> Result<(), PolicyDenied> {
    if !has_cap(db, caller, Capability::Kill) {
        return Err(deny(
            db,
            caller.as_str(),
            "kill",
            "lacks kill capability".into(),
        ));
    }
    if caller != session_owner {
        return Err(deny(
            db,
            caller.as_str(),
            "kill",
            format!("session is owned by {session_owner}, not the caller"),
        ));
    }
    Ok(())
}

/// `spawn(target)`: requires the `spawn` capability AND the target's cost class
/// must not exceed the caller's (spec §5.1). F3.1.
pub fn check_spawn(db: &Db, caller: &AgentId, target_cost: CostClass) -> Result<(), PolicyDenied> {
    if !has_cap(db, caller, Capability::Spawn) {
        return Err(deny(
            db,
            caller.as_str(),
            "spawn",
            "lacks spawn capability".into(),
        ));
    }
    let caller_cost = db
        .get_agent(caller)
        .ok()
        .flatten()
        .map(|c| c.cost_class)
        .unwrap_or(0);
    if target_cost > caller_cost {
        return Err(deny(
            db,
            caller.as_str(),
            "spawn",
            format!("target cost_class {target_cost} exceeds caller limit {caller_cost}"),
        ));
    }
    Ok(())
}

/// Test whether `path` resolves inside `root`. Both sides are normalized
/// lexically (`.`/`..` collapsed, made absolute) and compared by prefix, which
/// blocks `..` traversal escape deterministically. Symlink resolution is a
/// future hardening (a malicious symlink inside the worktree could still point
/// out); lexical normalization is the Phase 3 boundary (spec §5.1, §8.2
/// "canonicalize before policy").
pub fn path_within(path: &Path, root: &Path) -> bool {
    let root_n = lexical_abs(root);
    let target = if path.is_absolute() {
        lexical_normalize(path)
    } else {
        lexical_normalize(&root_n.join(path))
    };
    target.starts_with(&root_n)
}

/// Make a path absolute lexically (prepend cwd if relative) without fs access.
fn lexical_abs(p: &Path) -> PathBuf {
    if p.is_absolute() {
        lexical_normalize(p)
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        lexical_normalize(&cwd.join(p))
    }
}

/// Normalize `.`/`..` components lexically (no symlink resolution).
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use divan_core::{AgentCard, AgentStatus, AgentTool, DeliveryKind};

    fn card(id: &str, caps: Vec<Capability>) -> AgentCard {
        AgentCard {
            id: AgentId::new(id),
            tool: AgentTool::Claude,
            display_name: None,
            capabilities: caps,
            cost_class: 1,
            skills: vec![],
            delivery: vec![DeliveryKind::Mcp],
            multi_turn: true,
            status: AgentStatus::Idle,
            session_id: None,
            registered_at: 0,
        }
    }

    #[test]
    fn ungated_actions_pass_through() {
        let db = Db::open_in_memory().unwrap();
        assert!(check(&db, None, Action::Publish).is_ok());
        assert!(check(&db, Some(&AgentId::new("x")), Action::Claim).is_ok());
    }

    #[test]
    fn delegate_requires_delegate_capability() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("codex-1", vec![Capability::Read]))
            .unwrap();
        db.upsert_agent(&card(
            "claude-1",
            vec![Capability::Read, Capability::Delegate],
        ))
        .unwrap();
        assert!(check(&db, Some(&AgentId::new("codex-1")), Action::Delegate).is_err());
        assert!(check(&db, Some(&AgentId::new("claude-1")), Action::Delegate).is_ok());
    }

    #[test]
    fn subscribe_requires_read_and_denial_is_traced() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("noread", vec![])).unwrap();
        assert!(check(&db, Some(&AgentId::new("noread")), Action::Subscribe).is_err());
        let traced = db
            .timeline(&TraceId::new("policy"))
            .unwrap()
            .into_iter()
            .any(|e| e.event == TraceEventKind::PolicyDenied);
        assert!(traced);
    }

    #[test]
    fn unknown_agent_denied_for_gated_action() {
        let db = Db::open_in_memory().unwrap();
        assert!(check(&db, Some(&AgentId::new("ghost")), Action::Delegate).is_err());
    }

    // ---- execution-action matrix (F3.1) ----

    #[test]
    fn write_requires_capability_and_worktree_containment() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("writer", vec![Capability::Write]))
            .unwrap();
        db.upsert_agent(&card("reader", vec![Capability::Read]))
            .unwrap();
        let wt = tempfile::tempdir().unwrap();
        let inside = wt.path().join("src/x.rs");
        let outside = wt.path().join("../escape.rs");

        // No write capability -> denied.
        assert!(check_write(&db, &AgentId::new("reader"), &inside, wt.path()).is_err());
        // Write capability + inside worktree -> ok.
        assert!(check_write(&db, &AgentId::new("writer"), &inside, wt.path()).is_ok());
        // Write capability but escaping path -> denied (F3.1 unauthorized write).
        let err = check_write(&db, &AgentId::new("writer"), &outside, wt.path()).unwrap_err();
        assert!(err.reason.contains("outside"));
    }

    #[test]
    fn kill_requires_capability_and_ownership() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_agent(&card("owner", vec![Capability::Kill]))
            .unwrap();
        db.upsert_agent(&card("other", vec![Capability::Kill]))
            .unwrap();
        db.upsert_agent(&card("nokill", vec![Capability::Read]))
            .unwrap();
        let owner = AgentId::new("owner");
        // Owner with kill cap -> ok.
        assert!(check_kill(&db, &owner, &owner).is_ok());
        // Different caller -> denied (F3.1 unauthorized kill).
        assert!(check_kill(&db, &AgentId::new("other"), &owner).is_err());
        // No kill capability -> denied.
        assert!(check_kill(&db, &AgentId::new("nokill"), &AgentId::new("nokill")).is_err());
    }

    #[test]
    fn spawn_respects_cost_class_limit() {
        let db = Db::open_in_memory().unwrap();
        let mut c = card("boss", vec![Capability::Spawn]);
        c.cost_class = 3;
        db.upsert_agent(&c).unwrap();
        assert!(check_spawn(&db, &AgentId::new("boss"), 3).is_ok());
        assert!(check_spawn(&db, &AgentId::new("boss"), 5).is_err()); // exceeds limit
    }

    #[test]
    fn path_within_blocks_traversal_escape() {
        let root = tempfile::tempdir().unwrap();
        assert!(path_within(&root.path().join("a/b.rs"), root.path()));
        assert!(path_within(Path::new("a/b.rs"), root.path())); // relative -> joined
        assert!(!path_within(&root.path().join("../x"), root.path()));
        assert!(!path_within(Path::new("/etc/passwd"), root.path()));
    }
}
