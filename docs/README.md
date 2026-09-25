# Project documentation

Use these documents as project-specific working context. The implementation,
tests, migrations, and published OpenAPI document remain authoritative when a
detail changes; update the relevant documentation alongside that change.

## Core guides

- [Product requirements](PRD.md) — product goals, users, scope, and non-goals.
- [Security](SECURITY.md) — trust boundaries, credentials, authorization, and
  safe operations.
- [Code style](CODE_STYLE.md) — Rust, React/TypeScript, Tailwind, structure,
  testing, and verification conventions.
- [Database](DATABASE.md) — PostgreSQL model, SQLx migrations, and data safety.
- [API](API.md) — REST, authentication, events, and protocol conventions.

## Architecture and interface

- [Architecture](ARCHITECTURE.md) — component responsibilities and principal
  runtime flows.
- [Design system](DESIGN_SYSTEM.md) — frontend layout, visual conventions, and
  accessibility expectations.
- [Domain context](../CONTEXT.md) — project vocabulary and resource lifecycle.
- [Architecture decisions](adr/) — recorded decisions and their rationale.

## Operations and quality

- [Operations runbook](operations.md)
- [Worker setup](worker-setup.md)
- [Secret-store recovery](secret-store-recovery.md)
- [Quality gates](quality-gates.md)
- [Frontend testing](frontend-testing.md)
- [Deployment guide](../deploy/README.md)

The root [AGENTS.md](../AGENTS.md) tells coding agents which documents to read
for a given change.
