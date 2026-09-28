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
- Target responses include `agent_version` from the last authenticated heartbeat and `latest_agent_version` from the running controller build. The UI warns when they differ; onboarding and update workflows use the controller-reported version rather than a frontend constant.
- Telemetry sample times are interpreted relative to the authenticated heartbeat's `sent_at` and normalized to controller time. This preserves the rolling window when an agent and controller have modest clock skew; samples outside that window remain rejected.
- Linux agents collect telemetry every five seconds and include the rolling last 60 seconds with each 30-second heartbeat. The overlap lets the controller recover samples when a heartbeat is delayed or lost; duplicate normalized timestamps are ignored by persistence. The controller persists samples for 30 days and returns only the last 10 minutes for charts (about 120 samples at the normal interval). A daily cleanup removes expired records. Heartbeats include `missing_samples` for detected gaps and `partial: true` when samples are missing, rejected, or lack metrics, so operators can distinguish gaps from a quiet system.
- `GET /api/v1/telemetry-alerts` evaluates those stored samples. CPU warns above 90% for 5 minutes and becomes critical above 98% for 2 minutes; RAM warns above 90% for 5 minutes and becomes critical above 95% for 2 minutes; storage warns above 85% for 10 minutes and becomes critical above 95% for 5 minutes. A gap greater than 7 seconds breaks a sustained-load streak. A managed target also reports stale telemetry after 2 minutes without a fresh metric sample. Alerts include target identity, current value, threshold, firing time, and observation time; the UI polls every 15 seconds and retains observed recovery notices in browser storage.
- Linux agents also sample Docker CPU and memory usage every 10 seconds and include an overlapping 60-second window in each heartbeat. The controller validates IDs/timestamps, ignores duplicates, persists samples for 30 days, and returns the last 10 minutes. `GET /api/v1/targets/{target_id}/docker/telemetry` returns these per-container samples; Docker is optional and unsupported platforms simply return an empty series.
- Docker inventory lifecycle events are retained for 180 days (subject to the per-target 200-event cap) and pruned daily. The current container inventory is not subject to this event-history retention.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/action` accepts only `start`, `stop`, or `restart`, requires `confirmed: true` and the destructive/admin permission, verifies the container against a fresh agent inventory, and sends a fixed Docker subcommand plus a validated hexadecimal ID. It then best-effort refreshes the stored inventory so lifecycle changes are captured.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check` is a read-only registry check. It compares the running image's local config digest with the matching remote platform manifest and returns `current`, `update_available`, `pinned`, or `unknown`. It does not pull an image or recreate a container; registry authentication/errors yield `unknown` with a stable reason.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-apply` requires the destructive/admin permission and `{ "confirmed": true, "expected_remote_image_id": "sha256:…" }`. The controller performs a fresh successful registry check and requires the digest to match the supplied value. It is Linux-agent-only and only supports a single-replica Compose service with one config file inside its declared project directory. The agent verifies the Compose config hash and registry digest again, pulls the service image, then recreates only that service with `--no-deps`; plain Docker containers and changed/ambiguous Compose configurations are rejected. Updates are not automatically rolled back; see [`docker-image-update-recovery.md`](docker-image-update-recovery.md).
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
| Targets | `GET/POST /api/v1/targets`, `GET /api/v1/targets/{target_id}`, package inventory and host/Docker telemetry reads, active telemetry alerts at `GET /api/v1/telemetry-alerts`, Docker inventory read and confirmed discovery via a connected LXC agent at `/api/v1/targets/{target_id}/docker/discovery`, confirmed Docker lifecycle actions at `/api/v1/targets/{target_id}/docker/containers/{container_id}/action`, read-only registry checks at `/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check` |
| Enrollment and agents | `POST /api/v1/enrollments`, `GET /api/v1/enrollments/{enrollment_id}`, `POST /api/v1/agents/heartbeat` |
| Secrets | list/create, audit, metadata read, rotate, and revoke under `/api/v1/secrets` |
| Ansible workflows | enqueue, read, retry, and event reads under `/api/v1/ansible/jobs`; worker availability under `/api/v1/ansible/worker-availability` |
| Scheduling | list/create `/api/v1/schedules`; list/create `/api/v1/update-policies`; confirmed deletion of non-system policies at `DELETE /api/v1/update-policies/{policy_id}` |
| Planner and execution | scans, plans, confirmations, executions, results, abort, reconciliation, and safety reads under `/api/v1/containers`, `/api/v1/scans`, `/api/v1/plans`, and `/api/v1/executions` |
| Agent/container inventory | agent health/metrics/revoke, legacy Docker inventory and workload management under `/api/v1/containers/{container_id}`; manually registered LXC targets can discover Docker directly through their private-network agent endpoint |
| Discovery/documentation/events | `GET /api/v1/openapi.json`, authenticated `GET /api/v1/events` |

Linux package updates are limited to registered Debian/Ubuntu targets and
require an enabled update policy. In an `update_packages` request, the package
selector `packages: ["*"]` plans all installed packages with available updates;
it is permitted only when the policy has no package-name allowlist. Apply must
reference the matching successful plan and include explicit confirmation. The
workflow UI lists candidate updates from the selected targets' most recent
package inventories; selecting none means all updates for that target.

The endpoint group names reflect the current router and do not imply that a
route is available to every role. Do not add routes to this list until the
router and OpenAPI document expose them.

Deleting an update policy requires explicit confirmation and destructive
permission. The system-wide standard policy is protected, and a policy used by
an enabled recurring schedule cannot be deleted until that schedule is paused.
Deletion invalidates future applies for plans that reference that policy; the
workflow/job history remains available.

`GET /api/v1/ansible/worker-availability` reports heartbeat freshness and the
latest artifact-manifest probe (`artifact_store_available` and
`artifact_store_checked_at`). A `false` artifact value means the worker is
running but its configured versioned manifest endpoint did not return a
successful HTTP response on the last probe.

For registered LXC targets, `POST /api/v1/targets/{target_id}/docker/discovery`
persists each successful Docker inventory snapshot when PostgreSQL is enabled.
`GET` on the same path returns the latest successful snapshot after reload; a
failed later probe does not erase previously discovered containers. In
memory-only development mode snapshots remain available until the controller
restarts. The agent uses stable discovery reasons such as
`docker_permission_denied` for Docker-socket access failures and
`docker_cli_unavailable` when its Docker CLI cannot be launched; raw stderr is
not returned to the client. Container entries may also include optional
`created_at`, `started_at`, `image_id`, `restart_count`, `health`, and
`oom_killed` metadata
reported by Docker `inspect`. Older agents or containers without a healthcheck
may omit those values; clients must render them as unavailable rather than as
zero or healthy. The response also includes up to 200 recent container change
events (`added`, `removed`, image/state/health changes, and increased restart
count or an observed OOM-killed state), ordered from oldest to newest. Events
are generated only from successful snapshots; elapsed human-readable status
text is not treated as a change.

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
