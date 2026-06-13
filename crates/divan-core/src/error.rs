//! Shared error taxonomy (AGENTS.md "Clear error handling").
//!
//! `divan-core` defines the domain error; storage/IO crates wrap their own
//! errors and convert into this where they cross the domain boundary.

use crate::task::TaskState;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DivanError>;

/// Domain-level error for Divan core operations.
#[derive(Debug, Error)]
pub enum DivanError {
    /// A message/artifact summary violated the length contract (K4, spec §4).
    #[error("invalid summary: {reason}")]
    InvalidSummary { reason: String },

    /// An illegal task state transition was attempted (spec §4.2).
    #[error("illegal task transition: {from:?} -> {to:?}")]
    IllegalTransition { from: TaskState, to: TaskState },

    /// A capability check failed (K8, spec §5.1).
    #[error("policy denied: agent {agent} lacks capability for {action}")]
    PolicyDenied { agent: String, action: String },

    /// A referenced entity was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// A value failed validation (generic).
    #[error("invalid input: {0}")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_error_displays_states() {
        let e = DivanError::IllegalTransition {
            from: TaskState::Open,
            to: TaskState::Done,
        };
        assert!(e.to_string().contains("Open"));
        assert!(e.to_string().contains("Done"));
    }
}
