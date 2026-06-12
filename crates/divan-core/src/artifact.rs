//! Artifact metadata domain types (spec §4 `artifacts`, §4.3, K4/K8/K9).
//!
//! The artifact *store* (content-addressed file IO + BLAKE3 hashing) lives in
//! `divan-db`; this module holds only the metadata record and the path layout
//! rule so `divan-core` stays dependency-light.

use crate::ids::{AgentId, ArtifactRef};
use serde::{Deserialize, Serialize};

/// Artifact metadata row (spec §4 `artifacts`). Content lives on disk at
/// [`ArtifactMeta::path`]; this is the SQLite-backed metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMeta {
    /// BLAKE3 hex digest (spec §4 `artifacts.ref`).
    pub artifact_ref: ArtifactRef,
    /// Relative path under the artifact root, e.g. `.divan/artifacts/ab/cdef...`.
    pub path: String,
    pub mime: Option<String>,
    pub bytes: u64,
    /// Producer-written summary, `<= MAX_SUMMARY_LEN` (spec §4, K4).
    pub summary: Option<String>,
    pub created_by: Option<AgentId>,
    pub created_at: i64,
}

/// Build the content-addressed relative path for a digest:
/// `{first_two}/{rest}` under the artifact root (spec §4.3).
///
/// Returns `None` if the digest is too short to split (defensive; BLAKE3 hex
/// digests are 64 chars).
pub fn shard_path(digest: &str) -> Option<String> {
    if digest.len() < 3 {
        return None;
    }
    let (head, tail) = digest.split_at(2);
    Some(format!("{head}/{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_splits_first_two() {
        assert_eq!(shard_path("abcdef123").as_deref(), Some("ab/cdef123"));
    }

    #[test]
    fn shard_rejects_too_short() {
        assert_eq!(shard_path("a"), None);
    }
}
