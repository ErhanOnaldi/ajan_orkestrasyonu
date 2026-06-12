-- Divan initial schema (spec §4, migrations/0001_initial.sql).
-- Carries spec v0.3 schema verbatim, including tasks.max_runtime_secs and
-- messages.origin_message_id. message_deliveries is NOT created in v1 (P0.2).
-- SQLite runtime is the source of truth (AGENTS.md K7/§4); WAL set at runtime.

PRAGMA foreign_keys = ON;

-- Agent registry (A2A AgentCard adaptation).
CREATE TABLE IF NOT EXISTS agents (
  id            TEXT PRIMARY KEY,        -- "claude-1", "codex-rev"
  tool          TEXT NOT NULL,           -- claude|codex|copilot|agy|opencode
  display_name  TEXT,
  capabilities  TEXT NOT NULL,           -- JSON: ["read","write","delegate"]
  cost_class    INTEGER NOT NULL,        -- 1 cheap .. 5 expensive
  skills        TEXT,                    -- JSON: ["csharp","review","sql"]
  delivery      TEXT NOT NULL,           -- JSON: ["hook","mcp","resume"]
  multi_turn    INTEGER NOT NULL,        -- 0|1 (spec §3.5: agy=0)
  status        TEXT NOT NULL,           -- idle|busy|offline
  session_id    TEXT,
  registered_at INTEGER NOT NULL
);

-- Tasks (A2A Task lifecycle).
CREATE TABLE IF NOT EXISTS tasks (
  id               TEXT PRIMARY KEY,
  parent_id        TEXT REFERENCES tasks(id),
  kind             TEXT NOT NULL,        -- implement|review|test|plan|research|analyze|report
  title            TEXT NOT NULL,
  spec_ref         TEXT,                 -- artifact reference (K4)
  state            TEXT NOT NULL,        -- open|claimed|working|review|done|failed|cancelled
  assignee         TEXT REFERENCES agents(id),
  worktree         TEXT,                 -- path or NULL (read-only task)
  max_runtime_secs INTEGER,              -- NULL = kind-based config default (watchdog)
  trace_id         TEXT NOT NULL,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tasks_state ON tasks(state);
CREATE INDEX IF NOT EXISTS idx_tasks_trace ON tasks(trace_id);

CREATE TABLE IF NOT EXISTS task_deps (
  task_id    TEXT NOT NULL REFERENCES tasks(id),
  blocked_by TEXT NOT NULL REFERENCES tasks(id),
  PRIMARY KEY (task_id, blocked_by)
);

-- Messages: pointer + summary, NO heavy content (K4).
-- Fan-out (P0.2): to_agent=NULL rows never enter the delivery queue; per-target
-- rows are copied with origin_message_id linking back to the source broadcast.
CREATE TABLE IF NOT EXISTS messages (
  id                TEXT PRIMARY KEY,
  origin_message_id TEXT REFERENCES messages(id),
  from_agent        TEXT NOT NULL,
  to_agent          TEXT,               -- NULL only on the source broadcast row
  kind              TEXT NOT NULL,      -- handoff|review_done|question|status|alert
  summary           TEXT NOT NULL CHECK(length(summary) <= 400),
  payload           TEXT,               -- small structured JSON
  artifact_ref      TEXT,               -- heavy content goes here
  task_id           TEXT,
  trace_id          TEXT,
  created_at        INTEGER NOT NULL,
  delivered_at      INTEGER             -- NULL = queued (batching, K5)
);

CREATE INDEX IF NOT EXISTS idx_messages_to_undelivered
  ON messages(to_agent) WHERE delivered_at IS NULL AND to_agent IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_messages_origin ON messages(origin_message_id);

CREATE TABLE IF NOT EXISTS artifacts (
  ref        TEXT PRIMARY KEY,          -- blake3 hash
  path       TEXT NOT NULL,             -- .divan/artifacts/ab/cdef...
  mime       TEXT,
  bytes      INTEGER,
  summary    TEXT,                      -- producer-written, <= 400
  created_by TEXT,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS subscriptions (
  agent_id   TEXT NOT NULL,
  event_kind TEXT NOT NULL,
  filter     TEXT,
  PRIMARY KEY (agent_id, event_kind)
);

-- Observability (K10).
CREATE TABLE IF NOT EXISTS trace_events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  trace_id    TEXT NOT NULL,
  span_id     TEXT,
  parent_span TEXT,
  agent_id    TEXT,
  event       TEXT NOT NULL,            -- spawn|tool_call|file_edit|msg_sent|policy_denied|...
  data        TEXT,                     -- JSON detail
  ts          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_trace_events_trace ON trace_events(trace_id, ts);

-- Conflict detection (hcom's 30s window pattern).
CREATE TABLE IF NOT EXISTS file_touches (
  path     TEXT NOT NULL,
  agent_id TEXT,
  task_id  TEXT,
  ts       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_touches_path ON file_touches(path, ts);
