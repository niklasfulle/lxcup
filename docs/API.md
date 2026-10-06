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

- Production uses local username/password accounts (no registration or email).
  The first PostgreSQL-backed startup creates `admin` / `admin` and sets a
  forced-password-change flag. `POST /api/v1/auth/login` sets an eight-hour
  `HttpOnly`, `SameSite=Strict` session cookie and does not return its secret in
  JSON. Production also marks it `Secure`. The same-origin frontend can restore
  the session after a reload; state-changing requests echo a separate CSRF
  cookie in `X-CSRF-Token`. Logout revokes the current session and clears both
  cookies. Password change/reset and account disable/role changes revoke
  affected sessions. Legacy bearer authentication remains available when
  account authentication is disabled.
- Supported human roles are `admin` and `user`. The server enforces role checks
  on every protected request. User administration, secret management, and the
  user audit API are Admin-only; navigation visibility is not an authorization
  boundary.
- Successful and failed account login, logout, password changes, user
  administration, and authenticated resource mutations are recorded with a
  stable action identifier, outcome, actor, and request ID. Audit metadata
  omits request bodies and credential values. Denied authenticated attempts
  to use Admin-only or destructive APIs are recorded. User-management failures
  are recorded as attempted actions; successful operations use action-specific
  identifiers.
- `GET /api/v1/auth/audit` returns `{ events, total, limit, offset }` and accepts
  `limit` (1–100), `offset`, `actor_username`, `action`, `resource`, `since`,
  and `until` (RFC 3339 timestamps; `until` is exclusive). Results are ordered
  newest first; string filters use case-insensitive substring matching.
- `GET /api/v1/admin/support-diagnostics` is Admin-only. It returns a bounded
  JSON support bundle with controller/agent/worker versions, worker and
  artifact-store availability, target agent platform/heartbeat state, and the
  50 newest workflow summaries in the selected `days` window (1–30, default
  7). It includes only allowlisted mode summaries;
  raw stdout/stderr, secret values, credentials, and environment variables are
  excluded. The UI downloads it locally and does not upload it anywhere.
- The agent heartbeat authenticates separately using the target's agent token.
- Target names are display labels and need not be unique. Use the target UUID
  to address a resource; when presenting target choices, include kind and
  address to distinguish resources with the same name.
- Windows targets use the `agent` transport and do not have a deployment
  credential or WinRM connection. The target page downloads one PowerShell
  setup script; when run locally it downloads the requested version's manifest
  and Windows amd64 executable directly from `/agent/<version>/`, then checks
  version, platform, PE32+ architecture, and SHA-256 before installation. It
  prompts for the agent token instead of putting it in the script, command
  argument, or browser history. After the initial local install, the existing
  `update_agent` workflow can check, plan, and apply an agent update through the
  authenticated outbound Windows-agent queue. The agent downloads and verifies
  the same versioned artifact, stages it beside the running binary, switches
  the Windows service, verifies the new local health/version, and restores the
  old service binary if activation fails. Windows workflow claim and result routes accept only
  Windows targets registered with the `agent` transport; legacy WinRM targets
  cannot use the local agent workflow API.
- A Windows agent collects installed software and available upgrades locally
  with Microsoft's `Microsoft.WinGet.Client` PowerShell module, then includes
  the bounded inventory in its authenticated heartbeat. Windows setup ensures
  PowerShell 7.4 or later is installed (using WinGet when needed), then installs
  that module for all users from PowerShell Gallery with PSResourceGet so the
  `LocalSystem` service can use it without the legacy NuGet provider; WinGet
  CLI commands are not supported in that service context.
  Inventory entries from the `winget` and `msstore` sources are retained with
  their available-update metadata. Installed entries whose `ARP\` or `MSIX\`
  identifiers cannot be correlated to a WinGet source are retained as
  read-only inventory rows, with candidate updates removed. Other invalid or
  unsupported individual entries are discarded. An invalid or stale optional
  inventory snapshot is ignored without rejecting the heartbeat, so agent
  connection state and system telemetry continue to update. A requested Windows
  inventory workflow refreshes the full package list locally and sends it
  immediately in a heartbeat without first running the separate update-listing
  command. Inventory failures use stable, allowlisted diagnostic codes for an
  unavailable WinGet module, timeout, excessive output, or invalid package data;
  arbitrary agent stderr is not included in workflow summaries. Windows
  update plans are refreshed with `Get-WinGetPackage` and its available-update
  metadata. After an approved plan
  and explicit Apply confirmation, the agent updates each exact selected ID
  with `Update-WinGetPackage`; wildcard/`--all` updates are rejected. These
  module operations are limited to machine-wide packages visible to the
  `LocalSystem` service. The Windows agent claims allowlisted jobs over its outbound
  authenticated connection at `POST /api/v1/agents/workflows/claim` and reports
  results to `POST /api/v1/agents/workflows/{job_id}/result`. The controller
  never connects to Windows through WinRM or runs winget remotely.
- Target responses include `agent_version` and `agent_last_seen_at` from the last authenticated heartbeat and `latest_agent_version` from the running controller build. The UI warns when versions differ and uses the real heartbeat timestamp for stale-connection warnings; onboarding and update workflows use the controller-reported version rather than a frontend constant.
- Telemetry sample times are interpreted relative to the authenticated heartbeat's `sent_at` and normalized to controller time. This preserves the rolling window when an agent and controller have modest clock skew; samples outside that window remain rejected.
- Linux agents collect telemetry every five seconds and include the rolling last 60 seconds with each 30-second heartbeat. The overlap lets the controller recover samples when a heartbeat is delayed or lost; duplicate normalized timestamps are ignored by persistence. The controller persists samples for 30 days and returns only the last 10 minutes for charts (about 120 samples at the normal interval). A daily cleanup removes expired records. Heartbeats include `missing_samples` for detected gaps and `partial: true` when samples are missing, rejected, or lack metrics, so operators can distinguish gaps from a quiet system.
- Windows agents also attempt host telemetry collection every five seconds and include the rolling samples in authenticated heartbeats. CPU, memory, fixed-disk storage, network counters, and process count are collected independently, so an unavailable Windows collector leaves only its own metric absent instead of discarding the rest of the sample.
- `GET /api/v1/telemetry-alerts` evaluates those stored samples. CPU warns above 90% for 5 minutes and becomes critical above 98% for 2 minutes; RAM warns above 90% for 5 minutes and becomes critical above 95% for 2 minutes; storage warns above 85% for 10 minutes and becomes critical above 95% for 5 minutes. A gap greater than 7 seconds breaks a sustained-load streak. A managed target also reports stale telemetry after 2 minutes without a fresh metric sample. Alerts include target identity, current value, threshold, firing time, and observation time; the UI polls every 15 seconds and retains observed recovery notices in browser storage.
- Linux agents also sample Docker CPU and memory usage every 10 seconds and include an overlapping 60-second window in each heartbeat. The controller validates IDs/timestamps, ignores duplicates, persists samples for 30 days, and returns the last 10 minutes. `GET /api/v1/targets/{target_id}/docker/telemetry` returns these per-container samples; Docker is optional and unsupported platforms simply return an empty series.
- Docker inventory lifecycle events are retained for 180 days (subject to the per-target 200-event cap) and pruned daily. The current container inventory is not subject to this event-history retention.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/action` accepts only `start`, `stop`, or `restart`, requires `confirmed: true` and the destructive/admin permission, verifies the container against a fresh agent inventory, and sends a fixed Docker subcommand plus a validated hexadecimal ID. It then best-effort refreshes the stored inventory so lifecycle changes are captured.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check` is a read-only registry check. It compares the running image's local config digest with the matching remote platform manifest and returns `current`, `update_available`, `pinned`, or `unknown`. It does not pull an image or recreate a container; registry authentication/errors yield `unknown` with a stable reason.
- `POST /api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-apply` requires the destructive/admin permission and `{ "confirmed": true, "expected_remote_image_id": "sha256:…" }`. The controller performs a fresh successful registry check and requires the digest to match the supplied value. It is Linux-agent-only and only supports a single-replica Compose service with one config file inside its declared project directory. The agent verifies the Compose config hash and registry digest again, pulls the service image, then recreates only that service with `--no-deps --wait`; it verifies the resulting service is running and returns its observed health status (`null` means the Compose service has no healthcheck). Plain Docker containers and changed/ambiguous Compose configurations are rejected. An unsuccessful post-update state check is reported as a failure and does not imply rollback; see [`docker-image-update-recovery.md`](docker-image-update-recovery.md).
- The event stream at `GET /api/v1/events` uses the authenticated stream
  request; do not put tokens in the URL.
- Health endpoints and metrics have their own exposure rules; production Caddy
  intentionally does not expose metrics publicly. See
  [`operations.md`](operations.md).

## Endpoint groups

### Human permission matrix

| Capability | Unauthenticated | User | Admin |
| --- | --- | --- | --- |
| Login, public health, agent heartbeat (agent token) | Allowed | Allowed | Allowed |
| Read managed resources, telemetry, inventory, workflows, schedules, and policies | Denied | Allowed | Allowed |
| Start permitted non-destructive workflows and configure schedules/policies | Denied | Allowed | Allowed |
| Delete targets, destructive Docker actions, or destructive workflow operations | Denied | Denied | Allowed |
| Manage users, secrets, or read user audit events | Denied | Denied | Allowed |

The forced-password-change session is limited to session status, logout, and
password change. Every other application API is denied until rotation succeeds.

This index is intentionally grouped rather than duplicating generated schemas.
Consult OpenAPI for exact payloads and response codes.

| Area | Routes |
| --- | --- |
| Health and metrics | `GET /health/live`, `GET /health/ready`, `GET /metrics` |
| Authentication | `GET /api/v1/auth/status`, `POST /api/v1/auth/login`, `GET /api/v1/auth/session`, `POST /api/v1/auth/logout`, `POST /api/v1/auth/password` |
| Admin | Admin-only user management under `/api/v1/users`, user activity under `GET /api/v1/auth/audit` |
| Targets | `GET/POST /api/v1/targets`, `GET /api/v1/targets/{target_id}`, confirmed Admin-only `DELETE /api/v1/targets/{target_id}`, package inventory and host/Docker telemetry reads, active telemetry alerts at `GET /api/v1/telemetry-alerts`, Docker inventory read and confirmed discovery via a connected LXC or Linux-server agent at `/api/v1/targets/{target_id}/docker/discovery`, confirmed Docker lifecycle actions at `/api/v1/targets/{target_id}/docker/containers/{container_id}/action`, read-only registry checks at `/api/v1/targets/{target_id}/docker/containers/{container_id}/image-update-check` |
| Enrollment and agents | `POST /api/v1/enrollments`, `GET /api/v1/enrollments/{enrollment_id}`, `POST /api/v1/agents/heartbeat`; Windows agents also claim local allowlisted workflows at `POST /api/v1/agents/workflows/claim` and report results at `POST /api/v1/agents/workflows/{job_id}/result` |
| Secrets | list/create, audit, metadata read, rotate, and revoke under `/api/v1/secrets` |
| Ansible workflows | enqueue, read, retry, and event reads under `/api/v1/ansible/jobs`; worker availability under `/api/v1/ansible/worker-availability` |
| Scheduling | list/create `/api/v1/schedules`; list/create `/api/v1/update-policies`; confirmed deletion of non-system policies at `DELETE /api/v1/update-policies/{policy_id}` |
| Planner and execution | scans, plans, confirmations, executions, results, abort, reconciliation, and safety reads under `/api/v1/containers`, `/api/v1/scans`, `/api/v1/plans`, and `/api/v1/executions` |
| Agent/container inventory | agent health/metrics/revoke, legacy Docker inventory and workload management under `/api/v1/containers/{container_id}`; registered LXC and Linux-server targets can discover Docker directly through their private-network agent endpoint |
| Discovery/documentation/events | `GET /api/v1/openapi.json`, authenticated `GET /api/v1/events`, Admin-only `GET /api/v1/admin/support-diagnostics` |

Admins can list encrypted backups with `GET /api/v1/admin/backups`, explicitly
create one using `POST` with `{ "confirmed": true, "passphrase": "…" }`, and
stream a retained archive from `GET /api/v1/admin/backups/{backup_id}`. The
passphrase must have at least 12 characters and is used with age's standard
passphrase encryption; it is never stored by lxcup. Creation requires `pg_dump`,
a configured PostgreSQL `DATABASE_URL`, and the encrypted secret-store
directory. The encrypted archive is kept in the dedicated backup volume; the
passphrase, secret master key, and live `.env` are excluded. Compose files and
`.env.example` are included only as versioned deployment templates. Downloads
can be decrypted with `age --decrypt --output backup.tar <backup-file>`; age
prompts for the passphrase.

Admins can also schedule `create_backup` through `POST /api/v1/schedules`.
The request must include `backup_secret_ref` pointing to an active, global
`backup_passphrase` Secret. Backup schedules have no targets or telemetry
thresholds, use UTC, and run no more frequently than hourly. Secret values and
their reference are not returned in schedule responses; the controller
resolves the current secret value when each run starts. The encrypted result
stays in the same retained backup list for manual download. Successful and
failed runs publish a `backup` status event to the Admin activity feed; event
payloads contain only the backup/schedule ID and state.

`GET /api/v1/admin/support-diagnostics?days=7` returns a versioned JSON
document with `schema_version`, generation/period timestamps, controller/agent/
worker/artifact versions, worker/artifact availability, bounded `resources`,
bounded `workflows`, and a `privacy` declaration. Each workflow contains its
registered operation, target identifier, mode/status/timestamps, failure
codes, and up to 10 sanitized summaries (500 characters each); the response
contains at most 250 resources, 50 workflows, and at most 100 recent source
events per workflow when reading PostgreSQL. The requested period is 1–30
days. It never includes raw worker stdout/stderr, secret values, process
environments, or key material, and the endpoint does not upload data.

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

When a workflow request or reconciliation is rejected because another job
owns the target lock, the `409 ansible_target_busy` error includes an optional
`error.blocking_job` object with that job's `id` and current `status`. The UI
links directly to the blocking workflow. A `reconcile_required` job keeps the
target locked until its explicit reconciliation finishes; the controller never
marks an interrupted Apply as complete just to release the lock.

Deleting an update policy requires explicit confirmation and destructive
permission. The system-wide standard policy is protected, and a policy used by
an enabled recurring schedule cannot be deleted until that schedule is paused.
Deletion invalidates future applies for plans that reference that policy; the
workflow/job history remains available.

`GET /api/v1/ansible/worker-availability` reports heartbeat freshness, the
worker's reported build version, and the latest artifact-manifest probe (`artifact_store_available` and
`artifact_store_checked_at`). A `false` artifact value means the worker is
running but its configured versioned manifest endpoint did not return a
successful HTTP response on the last probe.

For registered LXC and Linux-server targets with a connected Linux agent,
`POST /api/v1/targets/{target_id}/docker/discovery`
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

- Unknown browser routes render the custom frontend 404 page and return HTTP
  404 for document navigation. `/api` and `/api/*` remain on the API proxy and
  are never rewritten to the frontend shell.
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
