# Divan — Launch Kit (F5.4)

Draft copy for launch. **Posting is owner-driven** — nothing here is published
automatically. Fill the `<…>` placeholders (repo URL, demo GIF/asciinema link)
before posting. Keep every claim aligned with the README's
[Limitations](../README.md#limitations): **no "% token savings" claim.**

Prereqs before going live:
- [ ] Demo GIF recorded and embedded in the README (F5.3 TODO).
- [ ] README, install.sh verified on a clean macOS + Linux box (≤10 min).
- [ ] Repo public; `cargo install --path …` paths confirmed.

---

## Show HN

**Title:** `Show HN: Divan – a local-first hub so your CLI coding agents coordinate without burning tokens`

**Body:**

> I kept wiring Claude Code + Codex together for multi-step work and watched the
> token bill balloon — not from the actual coding, but from *coordination*:
> agents polling "any messages?", broadcasting everything to everyone, passing
> full file contents around in context. All of that gets re-tokenized every turn.
>
> Divan is a single Rust daemon that does the coordination in deterministic code
> instead. Queuing, routing, locking, dependency resolution — none of it touches
> an LLM. The model is used only for code/review/plan. Messages carry a pointer +
> a ≤400-char summary (heavy content lives in a content-addressed artifact store);
> they're batched and injected at the turn boundary so the prompt-cache prefix
> survives. Every write-capable task runs in its own git worktree, merge is always
> manual, and every decision is traced.
>
> v1 runs the write→review flow on real claude + codex (macOS/Linux). I'm
> deliberately *not* claiming a token-savings percentage — it reports proxy
> measures only. Design rationale (10 binding decisions, K1–K10) and ADRs are in
> the repo.
>
> Repo: <REPO_URL>  ·  Demo: <DEMO_LINK>
>
> Would love feedback on the capability/policy model and the adapter boundary.

---

## r/ClaudeCode (and r/LocalLLaMA variant)

**Title:** `I built a deterministic hub so Claude Code + Codex can work as a team without polling each other`

**Body:**

> Built **Divan**, a local-first orchestration hub in Rust. The idea: coordination
> between coding agents is pure bookkeeping (queues, routing, locks, deps) and
> shouldn't cost LLM tokens. So the hub does all of it deterministically and only
> calls the model for actual work.
>
> What it does today (v1, macOS/Linux):
> - **write → review** flow on real `claude` + `codex`
> - per-task **git worktree** isolation; merge is manual
> - **pointer messages** (≤400-char summary + artifact ref), batched at turn
>   boundaries via Claude Code hooks (push, not poll)
> - **capability-based permissions** (read/write/spawn/kill/delegate/broadcast) —
>   the opposite of the "total trust" model most multi-agent setups use
> - a `divan tui` screen + full traces
>
> Honest about scope: no "% savings" claim (proxy metrics only), Copilot/
> Antigravity adapters are limited, HTML trace export is v1.5.
>
> Repo + ADRs: <REPO_URL>

---

## awesome-agent-orchestrators — PR entry

> - **[Divan](<REPO_URL>)** — Local-first AI agent orchestration hub (Rust).
>   Deterministic coordination (zero LLM tokens for queuing/routing/locking),
>   pointer messages with turn-boundary batching, per-task git-worktree
>   isolation, and capability-based permissions. CLI + TUI + MCP server; runs the
>   write→review flow on real Claude Code / Codex. macOS + Linux.

---

## LinkedIn — article title ideas

A short series, one decision per post (each maps to a K-decision / ADR):

1. "Your multi-agent setup is paying the model to do bookkeeping" — K2, the
   zero-LLM-coordination thesis.
2. "Pointers, not payloads: why agent messages should be ≤400 characters" — K4.
3. "One git worktree per task: how Divan makes parallel agents impossible to
   collide" — K7.
4. "Least privilege for AI agents: capability-based permissions vs. total trust"
   — K8.
5. "Trace everything: making 'why did the agent do that?' an answerable question"
   — K10.

Each post: 1 problem, 1 decision, 1 code/trace screenshot, link to the repo.
