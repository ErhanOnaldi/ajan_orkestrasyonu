# Divan - Claude Code Context

Read AGENTS.md first.

AGENTS.md defines repository-wide rules.

This file contains Claude Code specific workflow instructions.

---

# Project Overview

Divan is a local-first AI orchestration hub written in Rust.

Current status:

Phase 0 / Phase 1 oriented development.

Primary goals:

- Validate adapter assumptions
- Build deterministic hub core
- Build write-review workflow
- Keep architecture aligned with spec

---

# Architecture References

Always consult:

docs/architecture/general_plan_and_architecture.md

and

docs/architecture/implementation_plan.md

before modifying architecture.

Do not rely on memory.

Use the documents.

---

# Working Style

For any task larger than a few files:

1. Read relevant architecture sections.
2. Produce a short implementation plan.
3. Identify affected modules.
4. Implement.
5. Add tests.
6. Run validation.
7. Summarize changes.

Never start coding immediately on large tasks.

---

# Development Priorities

Priority order:

1. Correctness
2. Architecture compliance
3. Tests
4. Simplicity
5. Performance

Never sacrifice architecture for speed.

---

# Rust Guidelines

Prefer:

- Tokio
- Serde
- Strong typing
- Explicit state machines
- Small modules

Avoid:

- Unnecessary macros
- Clever abstractions
- Runtime type tricks
- Unsafe code unless absolutely required

---

# Database Rules

SQLite is the runtime source of truth.

Schema changes require:

- Migration update
- Tests
- Documentation update

Do not modify runtime state outside repositories/store layer.

---

# Task Execution Rules

When implementing a feature:

- Determine phase ownership.
- Verify feature belongs to active phase.
- Check acceptance criteria.
- Implement only what acceptance requires.

Do not implement future-phase functionality.

---

# Adapter Development Rules

Adapters are isolated modules.

Never leak tool-specific logic into core orchestration code.

New tool support must be implemented through adapters.

Core should not know:

- Claude internals
- Codex internals
- Copilot internals
- Antigravity internals

Only normalized events.

---

# Message Rules

Messages:

- max summary length = 400
- carry pointers
- never carry large content

Large content belongs in artifacts.

Always.

---

# Testing Checklist

Before considering work complete:

- cargo fmt
- cargo clippy
- cargo test

Verify:

- state transitions
- policy behavior
- routing decisions
- persistence behavior

---

# Definition Of Done

A task is complete only if:

- Architecture compliant
- Tests added
- Trace events generated where required
- Documentation updated if necessary
- No future-scope functionality added

---

# When Unsure

Do not guess.

Read architecture documents.

If ambiguity remains:

Ask for clarification.
