# Code style and engineering conventions

Prefer readable, focused changes that follow the current project patterns.
Do not introduce a new framework or abstraction when an existing module already
solves the problem.

## Stack

- Rust workspace: edition 2024, Rust 1.85, `rustfmt`, Clippy, Tokio, Axum, and
  SQLx. Workspace versions and shared dependencies are declared in the root
  `Cargo.toml`.
- Frontend: React 19, TypeScript, React Router, TanStack Query, Vite, Vitest,
  and Tailwind CSS 3.4. The lockfile and package manifests are authoritative.
- Integration tests use PostgreSQL only through the explicit, isolated
  `DATABASE_TEST_URL` described in the README and test-support code.

## General rules

- Keep modules and components cohesive, names explicit, and control flow easy
  to follow. Prefer simple functions and domain types over broad utility layers.
- Keep source files below 800 lines where practical. Split by responsibility
  before a file grows beyond that size; do not create artificial fragments just
  to satisfy a count.
- Preserve domain terminology from [`../CONTEXT.md`](../CONTEXT.md), including
  the distinction between a managed target and a Docker workload discovered on
  a managed host.
- Reuse existing repository, query, error, logging, and validation patterns.
  Do not duplicate business rules in handlers, workers, and the frontend.
- Keep comments focused on why a non-obvious choice exists. Remove stale or
  misleading comments when behavior changes.

## Rust

- Put business rules in `lxcup-core` or the owning domain crate; keep API
  handlers focused on protocol translation, authorization, and orchestration.
- Use typed IDs, enums, and validated domain values at boundaries. Avoid
  converting back to unchecked strings once a value is validated.
- Use the established error types and structured `tracing` fields. Do not log
  secret material or silently discard errors that affect correctness.
- Avoid `unwrap`/`expect` on runtime input or I/O paths. They are acceptable
  only where an invariant is locally proven and failure is genuinely
  unrecoverable; explain non-obvious invariants.
- Keep database access in persistence repositories and use bound SQL values.
- Add unit tests beside domain logic and integration tests for database or
  cross-component behavior. Keep integration tests isolated from development
  and production databases.

## React, TypeScript, and Tailwind

- Use function components and explicit prop types. Keep data fetching in the
  existing query/API layers and render clear loading, error, empty, and success
  states.
- Put Tailwind utility classes on the JSX elements they style. Use the shared
  `cn` helper only for conditional class composition; do not build a central
  `ui.ts` class catalogue or move component styling into opaque string maps.
- Keep `styles.css` for global resets, theme tokens, and rules Tailwind cannot
  express cleanly—not as a parallel component stylesheet system.
- Reuse query keys and DTOs from the existing frontend modules. Do not fetch
  directly from components when an API/query helper already exists.
- Provide accessible labels for icon-only controls, visible keyboard focus,
  semantic headings, and usable responsive behavior.
- Use Vitest and Testing Library for behavior. Prefer role/label queries and
  test visible states and user outcomes rather than implementation details.

## Before handoff

Run focused tests first, then applicable workspace checks. Common commands:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd frontend
npm test -- --run
npm run build
```

For project coverage thresholds and database test prerequisites, follow
[`quality-gates.md`](quality-gates.md) and the root README. Do not claim a
PostgreSQL-backed check passed if integration tests were skipped due to a
missing test database URL.
