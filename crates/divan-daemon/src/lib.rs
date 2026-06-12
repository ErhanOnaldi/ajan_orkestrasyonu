//! `divan-daemon` — the hub process: lifecycle, scheduler, worktree manager,
//! and (F1.3+) the JSON-RPC IPC server. Module wiring lives here; the binary
//! (`main.rs`) is a thin entrypoint.

pub mod bus;
pub mod config;
pub mod lifecycle;
pub mod protocol;
pub mod rpc;
pub mod scheduler;
pub mod worktree;
