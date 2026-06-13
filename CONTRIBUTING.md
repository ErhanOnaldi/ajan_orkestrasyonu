# Contributing to Divan

Thanks for your interest. Divan is a spec-first project: the architecture is
deliberate and the design decisions (K1–K10) are a binding contract, not
suggestions. Please read this before opening a PR.

## Ground rules

1. **Read the architecture first.** Source of truth:
   - [`docs/architecture/general_plan_and_architecture.md`](docs/architecture/general_plan_and_architecture.md) — the spec, including the K1–K10 design contract (§2).
   - [`docs/architecture/implementation_plan.md`](docs/architecture/implementation_plan.md) — phase ownership and acceptance gates.
   - [`docs/adr/`](docs/adr/) — the locked decisions and their rationale.
2. **Don't deviate from K1–K10 silently.** Any change that requires bending one
   of the ten decisions must be raised (issue or ADR) *before* implementation,
   per spec §2.
3. **Stay in phase.** Don't implement future-phase functionality. Check which
   phase owns the feature and what its acceptance criteria are.
4. **Adapters stay isolated.** Tool-specific logic (Claude/Codex/Copilot/
   Antigravity internals) never leaks into core orchestration code — the core
   sees only normalized events. New tool support = a new adapter.

## Project layout

A single Cargo workspace, eight crates:

| Crate | Role |
|-------|------|
| `divan-core` | Domain types, normalized events, error taxonomy |
| `divan-db` | SQLite repositories/store layer (runtime source of truth) |
| `divan-trace` | Trace timeline + metrics (K10) |
| `divan-adapters` | Per-tool adapters (claude/codex/copilot/antigravity) |
| `divan-daemon` | Hub: lifecycle, JSON-RPC, scheduler, bus, policy, router, worktrees |
| `divan-cli` | `divan` — the user-facing CLI |
| `divan-mcp` | `divan-mcp` — rmcp stdio MCP server face |
| `divan-hooks` | Hook installer (merge-not-overwrite, backup) |
| `divan-tui` | `divan-tui` — ratatui observability screen |

## Before you push

The same checks CI runs (and CI must stay green):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Database changes require all three of: a migration update, tests, and a docs
update. Don't mutate runtime state outside the repositories/store layer.

### Tests

- **Unit + integration tests run with no external tools** — that's the default
  `cargo test --workspace`.
- **Real-CLI contract tests are env-gated** and off unless you opt in:

  ```sh
  DIVAN_TEST_CLAUDE=1 cargo test -p divan-adapters
  DIVAN_TEST_CODEX=1  cargo test -p divan-adapters
  # also: DIVAN_TEST_COPILOT=1, DIVAN_TEST_AGY=1
  ```

  These spend real tokens and need the actual CLI installed. CI never sets them,
  so a missing tool never fails CI.

When you state a test count in a PR or commit, state it from a fresh
`cargo test --workspace` at that exact commit — don't carry a stale number.

## Commit / PR conventions

- Keep commits focused; explain *why* in the body, not just *what*.
- If a change touches architecture, update the relevant doc/ADR in the same PR.
- Trace events must be generated where the spec requires them (K10).

## Definition of done

A change is complete only when it is architecture-compliant, has tests, emits
the required trace events, updates docs/ADRs if needed, and adds no
future-phase scope.
