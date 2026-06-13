//! `divan-db` — SQLite store, repositories, and the content-addressed artifact
//! store (spec §4, impl plan §3, §F1.2).
//!
//! SQLite is the source of runtime state (AGENTS.md K7/§4, WAL mode). The
//! `Connection` is held behind an `Arc<Mutex<_>>` so the async daemon can call
//! repository methods inside `tokio::task::spawn_blocking` (impl plan §3 — never
//! block the event loop on SQLite). Repository methods here are synchronous.

pub mod artifacts;
pub mod conflicts;
pub mod store;

mod agents;
mod messages;
mod subscriptions;
mod tasks;
mod trace_store;

pub use artifacts::ArtifactStore;
pub use conflicts::{ConflictStore, CONFLICT_WINDOW_MS};
pub use store::{Db, DbError, DbResult};

// Repository trait surfaces (impl plan §F1.2/§F2.4). Implemented for `Db`.
pub use agents::AgentStore;
pub use messages::MessageStore;
pub use subscriptions::SubscriptionStore;
pub use tasks::TaskStore;
pub use trace_store::TraceStore;
