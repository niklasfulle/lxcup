# API and agent protocol

The Rust controller exposes a JSON REST API under `/api/v1`, plus an
authenticated server-sent event stream. The router in
`crates/lxcup-server/src/lib.rs` and the generated document at
`GET /api/v1/openapi.json` are the exact sources of truth for request/response
schemas and route availability.

## Base paths and formats

- Local Compose API: `http://127.0.0.1:8080`.
- Production browser traffic must go through the configured TLS reverse proxy.
- JSON request/response bodies use UTF-8. IDs in path parameters are validated
  by the server; clients must not assume that UI validation is sufficient.
- Preserve `/api/v1` compatibility. A breaking protocol change requires an
  explicit versioning/migration decision.

## Authentication

- Human-facing protected endpoints use `Authorization: Bearer <token>` when
  authentication is enabled. Production requires authentication and distinct
  Admin, Operator, and Viewer credentials.
- The server enforces role checks. Do not infer a complete endpoint permission
  matrix from navigation visibility; check handlers and middleware.
- The agent heartbeat authenticates separately using the target's agent token.
- The event stream at `GET /api/v1/events` uses the authenticated stream
  request; do not put tokens in the URL.
- Health endpoints and metrics have their own exposure rules; production Caddy
  intentionally does not expose metrics publicly. See
  [`operations.md`](operations.md).

## Endpoint groups

This index is intentionally grouped rather than duplicating generated schemas.
Consult OpenAPI for exact payloads and response codes.

| Area | Routes |
| --- | --- |
| Health and metrics | `GET /health/live`, `GET /health/ready`, `GET /metrics` |
| Authentication | `GET /api/v1/auth/session`, `POST /api/v1/auth/logout` |
| Targets | `GET/POST /api/v1/targets`, `GET /api/v1/targets/{target_id}`, target package inventory and telemetry reads |
| Enrollment and agents | `POST /api/v1/enrollments`, `GET /api/v1/enrollments/{enrollment_id}`, `POST /api/v1/agents/heartbeat` |
| Secrets | list/create, audit, metadata read, rotate, and revoke under `/api/v1/secrets` |
| Ansible workflows | enqueue, read, retry, and event reads under `/api/v1/ansible/jobs`; worker availability under `/api/v1/ansible/worker-availability` |
| Scheduling | list/create `/api/v1/schedules`; list/create `/api/v1/update-policies` |
| Planner and execution | scans, plans, confirmations, executions, results, abort, reconciliation, and safety reads under `/api/v1/containers`, `/api/v1/scans`, `/api/v1/plans`, and `/api/v1/executions` |
| Agent/container inventory | agent health/metrics/revoke, Docker inventory, discovery, adoption, and workload management under `/api/v1/containers/{container_id}` |
| Discovery/documentation/events | `GET /api/v1/openapi.json`, authenticated `GET /api/v1/events` |

The endpoint group names reflect the current router and do not imply that a
route is available to every role. Do not add routes to this list until the
router and OpenAPI document expose them.

## Errors and observability

- Use the existing server error mapping and JSON error shape; do not create
  endpoint-specific error formats without a deliberate compatibility reason.
- Return client-safe messages. Keep stack traces, database internals, and
  secret values in neither responses nor logs.
- Use appropriate HTTP status codes: 400 for invalid requests, 401 for missing
  or invalid authentication, 403 for insufficient permission, 404 for missing
  resources, 409 for conflicting state, and 5xx for unexpected server errors.
  The actual handler contract takes precedence for existing routes.
- Preserve request IDs, job IDs, event sequence, and resource IDs for
  correlation. Never use secret values as correlation fields.
- Job execution output is persisted as ordered job events. Redact credentials
  before appending worker output.

## Change protocol

When changing the API or agent protocol, update the handler, domain validation,
OpenAPI generation, frontend DTO/client, and tests together. For an agent
heartbeat or telemetry change, update the agent sender, server validator,
persistence representation, and compatibility tests. Document only stable
conventions here; keep detailed generated schemas in OpenAPI.
