# Divan

**A local-first orchestration hub that lets your CLI coding agents work as a team — without burning tokens to coordinate.**

Divan is a single Rust daemon that sits between you and the AI coding CLIs you
already use (Claude Code, Codex, Copilot CLI, …). It hands each task to the
right agent, isolates writes in per-task git worktrees, routes pointer-sized
messages between agents at turn boundaries, and traces every decision — all
coordinated by **deterministic code, not an LLM**.

> Status: **v1 (MVP)**. macOS + Linux. The end-to-end *write → review* flow runs
> on real `claude` + `codex`. See [Limitations](#limitations) for what is
> deliberately out of scope.

<!-- TODO(F5.3): record a 30–60s demo GIF of `divan run … && divan tui` and embed here:
     ![Divan write-review demo](docs/assets/demo.gif) -->
**Demo:** _GIF coming — for now, the 60-second tour is under [Quickstart](#quickstart)._

---

## The problem

Multi-agent setups today coordinate *through the model*: agents poll "do I have
messages?", broadcast everything to everyone, and pass full file contents around
in context. Every one of those is re-tokenized on every turn. Coordination —
queuing, routing, locking, dependency resolution — is pure bookkeeping that does
not need an LLM at all, yet it's usually the most expensive part of the bill.

Divan moves all of that into a deterministic hub and keeps the model focused on
the only thing it's needed for: writing code, reviewing it, planning.

## Design decisions (K1–K10)

These ten decisions are the binding design contract. The full rationale lives in
[`docs/architecture/general_plan_and_architecture.md`](docs/architecture/general_plan_and_architecture.md);
the load-bearing ones each have an [ADR](docs/adr/).

| # | Decision | In one line |
|---|----------|-------------|
| **K1** | Envelope from A2A, delivery from hooks | Data model adapted from A2A (AgentCard/Task/Message/Artifact); delivery via hook injection + idle wake, not "agent = HTTP server". |
| **K2** | Zero LLM tokens for coordination | Queuing, routing, locking, dependency resolution run in the hub's deterministic code. The model is used only for model-worthy work. → [ADR-0002](docs/adr/0002-deterministic-hub.md) |
| **K3** | Push, never poll | Agents never call a tool to ask "any messages?"; the hub injects at the turn boundary or wakes an idle agent. |
| **K4** | Messages carry pointers, not content | Heavy content goes to the artifact store; the message carries a reference + a ≤400-char structured summary. → [ADR-0003](docs/adr/0003-pointer-messages.md) |
| **K5** | Batch at the turn boundary | Pending messages accumulate and are appended as one compact block at the *end* of context, preserving the prompt-cache prefix. |
| **K6** | Scoped subscription, no broadcast | Relevance filtering happens in the hub; an agent receives only events it subscribed to. |
| **K7** | One worktree per task | Every write-capable task runs in its own git worktree; merge is explicit, human-approved. → [ADR-0004](docs/adr/0004-worktree-isolation.md) |
| **K8** | Capability-based permissions | Each agent has a capability set (`read`/`write`/`spawn`/`kill`/`delegate`/`broadcast`); default is least privilege, not widenable at runtime. → [ADR-0005](docs/adr/0005-policy-model.md) |
| **K9** | Cost-aware routing | Task metadata + agent profile (model strength, cost class, history) → a router score. Starts as a YAML rule engine. |
| **K10** | Trace everything | Every task is a trace, every agent interaction a span; messages carry correlation IDs. |

## Architecture

```text
   you ── divan CLI ───────────────┐                      ┌─ Claude Code ─┐
                                    │   unix-socket        │  Codex        │ external
   divan tui ───────────────────────▶  JSON-RPC           │  Copilot CLI  │ agent CLIs
                                    │                      └───────────────┘
                          ┌─────────▼──────────────────────────────────┐
                          │                Divan daemon                 │
                          │                                             │
                          │  AgentCards   Scheduler     Message Bus     │
                          │  Router (K9)  (state m/c)   (K3·K4·K5·K6)   │
                          │  Policy Engine (K8)         Artifact Store  │
                          │  Trace Collector (K10)                      │
                          │                                             │
                          │        Adapters (normalized events)         │
                          │   claude · codex · copilot · antigravity    │
                          └─────────┬───────────────────────┬───────────┘
                                    │                        │
                            SQLite (source of truth)   per-task git worktrees (K7)
                                                        delivery: MCP tools + hooks (K1)
```

The daemon owns all state; the CLI and TUI are thin clients over its JSON-RPC
socket. Tool-specific quirks live entirely in **adapters** — the core only ever
sees normalized events.

## Install

Requires a Rust toolchain (`cargo`) **≥ 1.88** — get one at <https://rustup.rs>.
macOS or Linux.

```sh
git clone https://github.com/ErhanOnaldi/ajan_orkestrasyonu
cd ajan_orkestrasyonu
./install.sh            # builds + installs divan, divan-daemon, divan-mcp, divan-tui
# or, with Claude hooks wired up in one go:
./install.sh --with-hooks
```

The installer puts all four binaries in `~/.cargo/bin` (the `divan` CLI finds
its siblings there). If that dir isn't on your `PATH`, the installer tells you
the line to add.

Prefer to do it by hand?

```sh
cargo install --path crates/divan-cli --locked
cargo install --path crates/divan-daemon --locked
cargo install --path crates/divan-mcp --locked
cargo install --path crates/divan-tui --locked
```

### Hooks

Delivery (K1/K3) uses Claude Code hooks. Installing them **merges** into your
existing `~/.claude` config and writes a backup first — it never clobbers hooks
you already have:

```sh
divan install-hooks --tool claude     # also: --tool codex | --tool all
divan uninstall-hooks --tool claude   # restores from backup
```

### Uninstall

```sh
divan uninstall-hooks --tool claude
cargo uninstall divan-cli divan-daemon divan-mcp divan-tui
```

## Quickstart

The 60-second tour of the *write → review* flow (one agent implements in an
isolated worktree, a second reviews, you approve the merge):

```sh
divan up                                         # start the hub daemon
divan run "add a CHANGELOG.md" --repo ~/code/myproj   # write-review flow
divan tui                                        # watch it live (q to quit)

divan status                                     # agents + tasks at a glance
divan trace <task-id>                            # full timeline + proxy metrics
divan diff <task-id>                             # the worktree diff awaiting review
divan merge <task-id>                            # explicit, human-approved merge
divan cleanup <task-id>                          # drop the worktree (artifacts kept)
divan down                                        # stop the daemon
```

### CLI reference

| Command | What it does |
|---------|--------------|
| `divan up` / `down` | Start / stop the hub daemon |
| `divan status` | Show agents and tasks |
| `divan run <task> --repo <path> [--flow write-review]` | Run a flow end-to-end |
| `divan tui` | Interactive 4-panel observability screen (needs a real terminal) |
| `divan trace <id>` | Trace timeline + token/cost proxy metrics (task id or trace id) |
| `divan log [--task <id>] [--trace <id>]` | Raw trace log, optionally filtered |
| `divan agents` | List registered agents (id, tool, capabilities) |
| `divan messages [--agent <id>] [--task <id>]` | List pointer messages |
| `divan diff <task-id>` | Show a task's worktree diff |
| `divan merge <task-id>` | Merge a task's worktree branch (never automatic) |
| `divan cleanup <task-id>` | Remove a task's worktree (artifacts kept) |
| `divan install-hooks` / `uninstall-hooks` `--tool <claude\|codex\|all>` | Manage delivery hooks |
| `divan mcp print-config` | Print the MCP server config to register Divan's tools |
| `divan router explain <task-id>` | Explain the Cost Router's pick (rules + reasons) |

## Limitations

Honesty over hype. v1 deliberately does **not** include:

- **No "% token savings" claim.** Divan reports only *proxy* measures (message
  count, summary length, injection/turn counts, cost-class distribution). It
  does not assert a savings percentage it can't rigorously measure.
- **Adapter coverage varies.** `claude` and `codex` are first-class (real
  stream-json / `exec --json`). Copilot CLI is plain-text with a git-diff-derived
  edit view and a worktree-scoped write boundary. Antigravity is a degraded
  one-shot adapter.
- **macOS + Linux only.** No Windows in v1.
- **Merge is always manual.** Divan never merges a worktree for you.
- **HTML trace export is v1.5**, not v1. Use `divan trace <id>` (text) or the
  TUI for now.
- **The interactive TUI needs a real terminal.** In a pipe/CI, use
  `divan-tui --once` for a single headless frame.

## Roadmap

- **v1.5:** static HTML trace export (`divan trace export --format html`),
  router score correction from trace telemetry.
- **Later:** A2A gateway (the data model is already A2A-shaped, K1), broader
  adapter coverage.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Real-CLI contract tests are env-gated and off by default
(`DIVAN_TEST_CLAUDE=1`, `DIVAN_TEST_CODEX=1`, …), so the suite passes without any
external tool installed. See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © Erhan Önaldı
