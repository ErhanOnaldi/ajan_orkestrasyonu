//! Strongly-typed identifier newtypes (spec §4.1).
//!
//! Each ID is a thin wrapper over `String`. They are distinct types so the
//! compiler refuses to mix a `TaskId` where an `AgentId` is expected.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_newtype!(
    /// Agent registry id, e.g. `"claude-1"`, `"codex-review"`.
    AgentId
);
id_newtype!(
    /// Task id (also the root of a trace when it is a root task).
    TaskId
);
id_newtype!(
    /// Message id.
    MessageId
);
id_newtype!(
    /// Content-addressed artifact reference (BLAKE3 hex digest).
    ArtifactRef
);
id_newtype!(
    /// Trace id — the lifetime of one root task (K10).
    TraceId
);
id_newtype!(
    /// Span id — one agent session/interaction within a trace (K10).
    SpanId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_types_roundtrip_json() {
        let t = TaskId::new("task-7");
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, "\"task-7\"");
        let back: TaskId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn display_and_as_str_agree() {
        let a = AgentId::from("claude-1");
        assert_eq!(a.to_string(), "claude-1");
        assert_eq!(a.as_str(), "claude-1");
    }
}
