# Security

This document records the security rules implemented by lxcup. It complements
the controls in code and deployment configuration; it is not a substitute for
them.

## Security model and scope

lxcup is a self-hosted control plane for existing Linux servers, LXC guests,
Windows systems, and Docker workloads reported by managed agents. It does not
create virtual machines or LXC guests and does not use a Proxmox control-plane
API. Treat the controller, PostgreSQL, encrypted secret store, worker, artifact
service, browser, and managed hosts as separate trust boundaries.

The worker is intentionally limited to registered operations and playbooks.
It is not a general remote shell. The worker claims jobs from the database,
enforces target-level job exclusivity, resolves only secret references attached
to the target, and builds a temporary Ansible inventory and variables file.

## Authentication and authorization

- Production must run with `LXCUP_ENV=production` and required authentication
  enabled. Configure separate, strong Admin, Operator, and Viewer bearer tokens
  through deployment secrets; never use development placeholders.
- The server authorizes every protected request. Frontend visibility is not an
  authorization boundary. Administrative secret lifecycle operations and
  destructive actions require the corresponding server-side role.
- Use TLS for browser access in production. Do not expose bearer tokens over
  plaintext HTTP or place them in URLs.
- The frontend keeps the authenticated token in in-memory application state;
  do not persist it to Local Storage, session storage, or cookies.
- Agent heartbeats use the target's agent token, independently of user-session
  tokens. Rotate or revoke credentials using the supported secret lifecycle.

For the implemented role behavior and production requirements, see
[`operations.md`](operations.md) and [`../deploy/README.md`](../deploy/README.md).

## Secrets and configuration

- Never commit credentials, private keys, agent tokens, production connection
  strings, or secret-store contents. `.env` is local-only; `.env.example` may
  contain variable names and clearly fake placeholders only.
- Secret metadata and secret values are separate. PostgreSQL holds references
  and metadata; values are stored by the encrypted file-backed secret store.
  Protect `LXCUP_SECRET_MASTER_KEY` as a high-value key and keep recovery
  material separate from encrypted backups.
- Do not return secret values from list/read APIs or expose them to the browser
  after a write. Pass references across internal boundaries, not secret
  material, except at the component that must use it.
- Worker and agent logs must redact credentials and tokens. Never include
  secret values in job events, errors, metrics, audit metadata, or test output.
- Prefer explicit allowlists of environment variables, secret kinds, and
  workflow parameters over pass-through configuration.

## Inputs, database, and errors

- Validate untrusted data on the server even when the UI already validates it.
  Use the domain types and validation rules in `lxcup-core` rather than
  reimplementing weaker checks in handlers.
- Use SQLx parameter binding and repository methods. Do not build SQL by
  concatenating user-controlled values.
- Apply authorization before reading or mutating protected resources. Keep
  target identity, secret references, operation, mode, and parameters bound and
  validated server-side.
- Return actionable but sanitized errors. Do not send stack traces, SQL details,
  internal filesystem paths, credentials, or process environments to clients.
- Correlate server and worker diagnostics using request/job/target identifiers;
  identifiers are useful context but must never be treated as credentials.

## Workflow and supply-chain safety

- Only registered operations, playbooks, and fixed parameter shapes may run.
  Never introduce arbitrary command strings, user-supplied playbook paths, or
  unrestricted Ansible extra-vars.
- Mutating package operations follow scan → plan → explicit confirmation →
  apply. Preserve idempotency, target exclusivity, audit events, and
  reconciliation behavior.
- Agent deployment/update artifacts require an approved manifest, matching
  version, and validated SHA-256 before execution. Do not bypass verification
  to make a deployment succeed.
- SSH host keys must be verified using the registered known-hosts secret. Do
  not disable host-key checking as a general workaround.
- Keep the worker least-privileged and its secret-store mount read-only. Avoid
  expanding network exposure or container privileges without an explicit
  security review.

## Changes and verification

For a security-sensitive change, review the relevant API handler, domain rule,
worker/agent boundary, and tests—not only the UI. Run focused tests and the
applicable quality checks. SonarQube scans are run manually by the project
owner at the end of a task; agents must not initiate scans. Never weaken
authentication, validation, TLS, secret handling, or artifact verification
solely to make a test pass; use isolated test fixtures instead.
