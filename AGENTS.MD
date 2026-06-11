# Divan - AI Agent Project Manifest

This file is the authoritative AI-agent manifest for the Divan repository.

All coding agents (Claude Code, Codex, Copilot CLI, Antigravity CLI, OpenCode, future adapters) must follow the rules in this document.

---

# Project Purpose

Divan is a local-first AI agent orchestration hub.

It coordinates multiple coding agents through:

- Task scheduling
- Agent registry
- Message routing
- Artifact storage
- Git worktree isolation
- Capability-based permissions
- Cost-aware routing
- Trace collection

Divan is NOT:

- An AI model
- A chatbot
- An A2A gateway
- A web dashboard
- A cloud service

The hub itself is deterministic.

No LLM calls are allowed inside orchestration logic.

---

# Source Of Truth

Architecture documents are the only source of truth:

1. docs/architecture/general_plan_and_architecture.md
2. docs/architecture/implementation_plan.md

If code and documentation disagree:

Documentation is correct.

Update documentation first.

Then update code.

Never do the opposite.

---

# Non-Negotiable Design Decisions

The K1-K10 decisions from the architecture document are mandatory.

Agents must not redesign:

- Message model
- Task lifecycle
- Permission model
- Cost routing model
- Worktree strategy
- Trace model

without explicit human approval.

---

# Scope Discipline

Implement only the currently active phase.

Do not implement future phases early.

Examples:

Forbidden during Phase 1:

- Web dashboard
- A2A gateway
- Multi-machine support
- Telemetry learning router
- Tree-sitter locking
- HTML trace export
- Windows support

If an improvement belongs to a future phase:

Document it.
Do not implement it.

---

# Architecture Rules

Divan follows these principles:

1. Deterministic orchestration.
2. Pointer-based messages.
3. Push delivery.
4. Task-scoped worktrees.
5. Capability-based permissions.
6. Trace everything.
7. SQLite is the source of runtime state.
8. Artifact store contains large content.
9. Messages never contain large content.
10. Hub never performs reasoning with an LLM.

---

# Coding Rules

Prefer:

- Small focused modules
- Explicit types
- Deterministic behavior
- Clear error handling
- Exhaustive enums

Avoid:

- Hidden magic
- Global mutable state
- Implicit side effects
- Premature abstractions
- Framework-heavy designs

---

# Testing Requirements

Every significant feature must include:

- Unit tests
  or
- Integration tests

Required coverage areas:

- State transitions
- Policy checks
- Routing decisions
- Message delivery
- Artifact storage
- Worktree management

---

# Documentation Requirements

Architecture-affecting changes require:

- ADR update
  or
- Architecture document update

before implementation.

Spec-first development is mandatory.

---

# Human Authority

Agents may propose architecture changes.

Agents may not silently apply architecture changes.

Human approval is required.
