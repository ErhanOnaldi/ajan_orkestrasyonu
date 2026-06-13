//! Scoped-subscription model (K6, spec §3.6, §4 `subscriptions`, impl plan §F2.4).
//!
//! There is no broadcast-to-everyone (K6). An agent subscribes to an
//! `event_kind` (a [`crate::MessageKind`] string, or `"*"` for any) with an
//! optional [`EventFilter`]; the hub matches a broadcast message against
//! subscriptions and fans it out only to matching agents.

use crate::ids::AgentId;
use serde::{Deserialize, Serialize};

/// Wildcard event kind that matches any message kind.
pub const EVENT_KIND_ANY: &str = "*";

/// Additional equality constraints on a subscription (all present fields must
/// match). Kept deliberately small and token-conscious (K4/K6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_agent: Option<String>,
}

impl EventFilter {
    /// True if this filter accepts a message with the given fields. An absent
    /// constraint accepts anything; a present one must equal the message field.
    pub fn matches(&self, task_id: Option<&str>, from_agent: &str) -> bool {
        if let Some(want) = &self.task_id {
            if Some(want.as_str()) != task_id {
                return false;
            }
        }
        if let Some(want) = &self.from_agent {
            if want != from_agent {
                return false;
            }
        }
        true
    }

    /// True if the filter has no constraints (matches everything).
    pub fn is_empty(&self) -> bool {
        self.task_id.is_none() && self.from_agent.is_none()
    }
}

/// An agent's subscription to an event kind (spec §4 `subscriptions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub agent_id: AgentId,
    /// A [`crate::MessageKind`] string (e.g. `"question"`) or [`EVENT_KIND_ANY`].
    pub event_kind: String,
    /// Optional extra constraints; `None` means "no filter".
    pub filter: Option<EventFilter>,
}

impl Subscription {
    /// True if this subscription matches a broadcast message of `kind` from
    /// `from_agent` (optionally scoped to `task_id`).
    pub fn matches(&self, kind: &str, task_id: Option<&str>, from_agent: &str) -> bool {
        let kind_ok = self.event_kind == EVENT_KIND_ANY || self.event_kind == kind;
        if !kind_ok {
            return false;
        }
        match &self.filter {
            Some(f) => f.matches(task_id, from_agent),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(kind: &str, filter: Option<EventFilter>) -> Subscription {
        Subscription {
            agent_id: AgentId::new("codex-1"),
            event_kind: kind.into(),
            filter,
        }
    }

    #[test]
    fn kind_must_match_or_wildcard() {
        assert!(sub("question", None).matches("question", None, "claude-1"));
        assert!(!sub("question", None).matches("status", None, "claude-1"));
        assert!(sub("*", None).matches("anything", None, "claude-1"));
    }

    #[test]
    fn filter_constrains_task_and_sender() {
        let f = EventFilter {
            task_id: Some("impl-1".into()),
            from_agent: None,
        };
        let s = sub("question", Some(f));
        assert!(s.matches("question", Some("impl-1"), "claude-1"));
        assert!(!s.matches("question", Some("impl-2"), "claude-1"));
        assert!(!s.matches("question", None, "claude-1"));
    }

    #[test]
    fn empty_filter_matches_any_fields() {
        let s = sub("*", Some(EventFilter::default()));
        assert!(s.matches("alert", Some("t"), "x"));
        assert!(EventFilter::default().is_empty());
    }

    #[test]
    fn filter_json_omits_none() {
        let f = EventFilter {
            task_id: Some("t1".into()),
            from_agent: None,
        };
        assert_eq!(serde_json::to_string(&f).unwrap(), r#"{"task_id":"t1"}"#);
    }
}
