# Architecture

lxcup is a self-hosted, agent-first control plane for managing systems that
already exist. The control plane does not provision LXC guests and does not
depend on a Proxmox API.

## Runtime components

```text
Browser
  └─ React / TypeScript / Tailwind frontend
       └─ REST + authenticated SSE ──> Rust controller (Axum)
                                      ├─ PostgreSQL (SQLx migrations/repositories)
                                      ├─ encrypted secret store (metadata refs in DB)
                                      ├─ registered workflow jobs ──> Ansible worker
                                      │                              ├─ approved artifacts
                                      │                              └─ existing Linux/Windows targets
                                      └─ authenticated agent API <── Linux/Windows agents
                                                                     └─ Docker discovery on managed hosts
```

In development Compose, the Vite frontend, controller, worker, PostgreSQL, and
artifact service run as separate services. Production builds the frontend into
an Nginx image; Caddy terminates TLS and routes browser traffic to that
frontend, which forwards `/api/*` to the controller over the private Compose
network. Deployment
configuration and service wiring are documented in `compose.yaml` and
[`../deploy/README.md`](../deploy/README.md).

Human access uses PostgreSQL-backed local accounts and revocable eight-hour
sessions issued as HttpOnly cookies, with a separate CSRF token for
state-changing browser requests. Agent heartbeats remain separately
authenticated with per-target agent tokens. First startup seeds a
forced-rotation Admin account; public user registration and email recovery are
not part of the system.

The production frontend serves known client routes from the SPA shell and
returns that shell with HTTP 404 for unknown browser routes so the application
can render its custom not-found page. `/api` routes bypass the SPA fallback and
retain controller responses. The authenticated `/api/v1/events` stream is
proxied without response buffering.

## Rust workspace

- `lxcup-core` — domain entities, identifiers, validation, and state machines.
- `lxcup-server` — HTTP/SSE API, authentication/authorization, workflow
  coordination, and runtime service composition.
- `lxcup-persistence` — SQLx repositories, embedded PostgreSQL migrations, and
  test fixtures/support.
- `lxcup-ansible-worker` — atomically claims queued work and runs only
  registered operations.
- `lxcup-ansible` — workflow operation/parameter mapping and Ansible inventory
  behavior.
- `lxcup-agent` — installed service that reports identity, heartbeat, package
  inventory, telemetry, and Docker discovery data supported by its build.
- `lxcup-planner`, `lxcup-execution`, `lxcup-apt`, `lxcup-safety`,
  `lxcup-observability`, and `lxcup-secrets` — focused planning, execution,
  package, safety, metrics/logging, and encrypted secret-store responsibilities.
- `lxcup-cli` — operator command-line entry point; `lxcup-test-support` —
  isolated integration-test support.

Keep domain policy in the appropriate domain crate. HTTP handlers translate
protocol requests and coordinate services; persistence repositories own SQL;
the frontend does not become a policy authority.

## Principal flows

### Target onboarding

1. An operator registers an existing resource with connection details and
   references to required credential, known-host, and agent-token secrets.
2. The controller persists the target as pending and ensures there is one
   shared standard update policy covering every registered target: all
   packages, high maximum risk, and a full-day UTC maintenance window. Existing
   targets are backfilled at startup and when the policy list is read; new
   targets extend the shared policy. Its reserved ID makes this idempotent, and
   operator-defined policy settings are preserved.
3. The controller enqueues the registered deployment workflow when requested.
4. The worker claims that job exclusively for its target, resolves only the
   referenced secrets from the read-only encrypted store, validates the
   approved artifact manifest/version/hash, and runs a fixed playbook with a
   temporary inventory.
5. The installed agent authenticates to the controller and sends a heartbeat.
   A valid heartbeat advances lifecycle state to managed.
6. The controller can then enqueue health and package-inventory jobs. Their
   status and ordered logs appear in workflow history and activity UI.

### Workflow execution

```text
enqueue → validate/authorize → queued → atomic worker claim → checking/planned
        → applying (if allowed) → succeeded / failed / reconcile_required
```

Every meaningful step persists status and an ordered event. Mutating package
work is scan → plan → explicit confirmation → apply. After interruption during
apply, reconciliation is required before retrying. The worker reports
stdout/stderr only after credential redaction.

### Agent status and telemetry

Agents initiate authenticated heartbeats; the controller does not poll agents
to establish presence. Heartbeat freshness is computed from persisted agent
signals. Telemetry samples are associated with their target and exposed through
the target API; current UI depicts a rolling recent window. Docker containers
are workloads discovered by an agent on a managed Linux host, not standalone
connections. For registered LXC and Linux-server targets, the controller persists the latest
successful Docker inventory snapshot so it remains visible after a page reload
or controller restart. Successful snapshots also retain a bounded history of
container additions/removals, image/state/health transitions, restarts, and
observed OOM-killed state.

The controller derives telemetry alerts from retained samples. Sustained CPU,
RAM, and storage thresholds ignore short spikes and break across sample gaps;
a separate freshness alert appears when managed-target telemetry is older than
two minutes. The frontend polls the alert endpoint and retains observed
recovery notices and read state in browser storage.

### Target removal

An Admin can explicitly remove a registered target after all its active
workflows finish. The controller transactionally deletes target-scoped
inventory, telemetry, Docker snapshots, workflow history, and associated
events; recurring schedules and policy scopes are pruned, with empty records
removed. A minimal deletion audit tombstone remains. This removes the resource
from lxcup only: it does not connect to the host to uninstall its agent, and
secret-store records are retained because they may be shared.

## State and trust boundaries

- PostgreSQL is the source of truth for resource metadata, workflow state,
  ordered job events, inventory, telemetry, schedules, policies, and audit
  records.
- Secret values are stored separately in the encrypted secret store. The
  controller/worker share it through configured storage with role-appropriate
  access; PostgreSQL stores references and safe metadata.
- The worker is a constrained execution boundary: fixed operations and
  parameters, target exclusivity, approved manifests, temporary files, bounded
  execution, and sanitized logs.
- Agents are independently authenticated clients. A reported heartbeat or
  inventory is accepted only after server-side identity and input checks.
- The artifact service serves versioned agent artifacts and manifests. It is
  not an arbitrary job input channel.

For the current Linux agent release, the manifest lists separate `linux-amd64`
and `linux-arm64` binaries. The worker verifies each SHA-256 digest before
passing their local paths to Ansible; the target's gathered architecture selects
the binary. Older manifests without ARM64 remain usable on x86-64, but an ARM64
target is rejected before installation when its artifact is absent.

The worker probes its configured versioned manifest at startup and every
15 seconds thereafter. It records artifact availability with its two-second
worker heartbeat, logs availability transitions, and keeps processing
non-artifact workflows while the store is unavailable. Compose gates initial
worker startup on a healthy artifact service and restarts both containers after
unexpected process exit; the UI exposes the latest probe result. Compose does
not restart a live container solely because its healthcheck reports
`unhealthy`.

For schema, API, security, and operating procedures, use the linked focused
documents from [`README.md`](README.md).
