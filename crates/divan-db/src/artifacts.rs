//! Content-addressed artifact store (K4, spec §3.1, §4.3, impl plan §F1.4).
//!
//! Content is hashed with BLAKE3; the digest is the [`ArtifactRef`]. Files live
//! at `{root}/{ab}/{cdef...}` (spec §4.3). Metadata is in SQLite. Re-publishing
//! identical content is deduped (same ref → same path). Heavy content lives
//! here; messages only carry the ref (K4).

use crate::store::{Db, DbError, DbResult};
use divan_core::{shard_path, validate_summary, AgentId, ArtifactMeta, ArtifactRef};
use rusqlite::params;
use std::path::{Path, PathBuf};

/// Content-addressed artifact store rooted at `root` (e.g. `.divan/artifacts`).
pub struct ArtifactStore {
    db: Db,
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(db: Db, root: impl Into<PathBuf>) -> Self {
        Self {
            db,
            root: root.into(),
        }
    }

    /// Publish content, returning its ref. Deduped: identical content yields the
    /// same ref and does not rewrite the file. `summary` (if any) must satisfy
    /// the K4 length contract.
    pub fn put(
        &self,
        content: &[u8],
        mime: Option<&str>,
        summary: Option<&str>,
        created_by: Option<&AgentId>,
        created_at: i64,
    ) -> DbResult<ArtifactRef> {
        if let Some(s) = summary {
            validate_summary(s).map_err(DbError::Domain)?;
        }
        let digest = blake3::hash(content).to_hex().to_string();
        let rel =
            shard_path(&digest).ok_or_else(|| DbError::Integrity("digest too short".into()))?;
        let abs = self.root.join(&rel);

        // Write file if absent (dedupe). Content-addressed => identical bytes.
        if !abs.exists() {
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, content)?;
        }

        let conn = self.db.lock()?;
        conn.execute(
            "INSERT INTO artifacts (ref, path, mime, bytes, summary, created_by, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(ref) DO NOTHING",
            params![
                digest,
                rel,
                mime,
                content.len() as i64,
                summary,
                created_by.map(|a| a.as_str()),
                created_at,
            ],
        )?;
        Ok(ArtifactRef::new(digest))
    }

    /// Read artifact content by ref.
    pub fn get(&self, r: &ArtifactRef) -> DbResult<Vec<u8>> {
        let meta = self
            .get_meta(r)?
            .ok_or_else(|| DbError::NotFound(format!("artifact {r}")))?;
        let abs = self.safe_join(&meta.path)?;
        Ok(std::fs::read(abs)?)
    }

    /// Read artifact metadata by ref.
    pub fn get_meta(&self, r: &ArtifactRef) -> DbResult<Option<ArtifactMeta>> {
        let conn = self.db.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM artifacts WHERE ref = ?1")?;
        let mut rows = stmt.query_map([r.as_str()], |row| {
            Ok(ArtifactMeta {
                artifact_ref: ArtifactRef::new(row.get::<_, String>("ref")?),
                path: row.get("path")?,
                mime: row.get("mime")?,
                bytes: row.get::<_, i64>("bytes")? as u64,
                summary: row.get("summary")?,
                created_by: row
                    .get::<_, Option<String>>("created_by")?
                    .map(AgentId::new),
                created_at: row.get("created_at")?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Join a stored relative path under `root`, rejecting traversal
    /// (impl plan §F1.4 "Artifact path traversal mümkün değildir").
    fn safe_join(&self, rel: &str) -> DbResult<PathBuf> {
        let p = Path::new(rel);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(DbError::Integrity(format!("unsafe artifact path: {rel}")));
        }
        Ok(self.root.join(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (ArtifactStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open_in_memory().unwrap();
        (ArtifactStore::new(db, dir.path()), dir)
    }

    #[test]
    fn same_content_same_ref_deduped() {
        let (s, _d) = store();
        let r1 = s.put(b"hello divan", None, None, None, 0).unwrap();
        let r2 = s.put(b"hello divan", None, None, None, 1).unwrap();
        assert_eq!(r1, r2, "identical content => identical ref");
        assert_eq!(s.get(&r1).unwrap(), b"hello divan");
    }

    #[test]
    fn different_content_different_ref() {
        let (s, _d) = store();
        let a = s.put(b"one", None, None, None, 0).unwrap();
        let b = s.put(b"two", None, None, None, 0).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn oversize_summary_rejected() {
        let (s, _d) = store();
        let long = "x".repeat(401);
        assert!(s.put(b"c", None, Some(&long), None, 0).is_err());
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let (s, _d) = store();
        assert!(s.safe_join("../escape").is_err());
        assert!(s.safe_join("/etc/passwd").is_err());
        assert!(s.safe_join("ab/cdef").is_ok());
    }

    #[test]
    fn path_is_sharded_by_first_two() {
        let (s, _d) = store();
        let r = s.put(b"shard me", None, None, None, 0).unwrap();
        let meta = s.get_meta(&r).unwrap().unwrap();
        assert_eq!(&meta.path[2..3], "/", "ab/rest layout");
    }
}
