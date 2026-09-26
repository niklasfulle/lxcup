# lxcup

lxcup is a self-hosted control plane for managing updates and operational
workflows across Linux servers, LXC guests, Docker workloads, and Windows
systems. It combines a web dashboard, a Rust controller and agents, PostgreSQL,
and a restricted Ansible worker.

> **Project status:** lxcup is under active development. The core onboarding,
> agent deployment, heartbeat, health-check, package inventory, and workflow
> paths are implemented. Some inventory, telemetry, backup, security, and
> production-operations features are still being completed; see the open GitHub
> issues before relying on a capability in production.

## Components

- `crates/lxcup-core` — domain types and business rules
- `crates/lxcup-agent` — Linux and Windows agent service
- `crates/lxcup-server` — REST/SSE API and workflow controller
- `crates/lxcup-persistence` — PostgreSQL repositories and migrations
- `crates/lxcup-ansible-worker` — isolated worker for approved Ansible jobs
- `crates/lxcup-ansible` — registered SSH/WinRM workflows and inventory handling
- `crates/lxcup-planner` and `crates/lxcup-execution` — safe update planning and
  confirmed execution
- `crates/lxcup-safety` and `crates/lxcup-observability` — health checks,
  reboot detection, logging, and error handling
- `frontend/` — React, TypeScript, and Vite web application
- `ansible/` — approved playbooks and roles

PostgreSQL is the controller's source of truth. Agent credentials are sent as
bearer tokens; secret values are stored separately from secret metadata. The
worker accepts registered playbooks and fixed parameters rather than arbitrary
commands. lxcup manages existing systems: creating LXC guests or accessing a
Proxmox API is out of scope.

## Quick start

Requirements: Docker Compose for the full development stack. For running checks
directly, use the Rust version pinned in `rust-toolchain.toml` and Node.js/npm
compatible with the frontend lockfile.

1. Create a local environment file and set a development-only database password:

   ```powershell
   Copy-Item .env.example .env
   ```

2. Start the development stack:

   ```powershell
   docker compose up -d --build
   docker compose ps
   ```

3. Open the UI at <http://127.0.0.1:5173>. The API is available at
   <http://127.0.0.1:8080>; liveness and readiness endpoints are
   `/health/live` and `/health/ready`.

The Compose stack includes PostgreSQL, the controller, frontend, artifact
service, and Ansible worker. Review `.env.example` and `deploy/README.md` before
configuring a non-development deployment. Do not reuse development credentials
in production.

To follow logs or stop the stack while preserving database data:

```powershell
docker compose logs -f lxcup-server lxcup-worker frontend
docker compose down
```

`docker compose down -v` also removes the Compose volumes and their data.

## Backup and restore

Create an encrypted backup of PostgreSQL and the encrypted secret store with
Docker Compose and [age](https://age-encryption.org/):

```powershell
.\scripts\backup-stack.ps1 -AgeRecipient $env:LXCUP_BACKUP_AGE_RECIPIENT
```

Set `LXCUP_BACKUP_AGE_RECIPIENT` to the public age recipient first. Keep the
matching age identity and `LXCUP_SECRET_MASTER_KEY` in separate,
access-controlled storage; neither private key is included in the backup. Do
not store backups, identities, or master keys in the repository. Configure an
external schedule and retention policy appropriate to your recovery objectives.

To validate a backup, install `age`, PostgreSQL client tools (`pg_restore` and
`psql`), and the Rust toolchain. Configure `DATABASE_TEST_URL` for an isolated
database whose name includes `test` or `restore`, provide the age identity and
the externally managed `LXCUP_SECRET_MASTER_KEY`, then run:

```powershell
.\scripts\restore-backup-test.ps1 `
  -BackupFile ".\backups\lxcup-<timestamp>.tar.age" `
  -AgeIdentity "$env:LXCUP_BACKUP_AGE_IDENTITY" `
  -ConfirmIsolatedDatabase
```

This test restores into the explicitly supplied database, applies migrations,
and validates database references, audit data, and secret-store readability.
It is destructive to that database: use a disposable restore/test database,
never a production database. See
[`docs/secret-store-recovery.md`](docs/secret-store-recovery.md) for the
recovery procedure and recovery objectives.

## Onboarding and agent deployment

Register a target in the UI, configure its SSH or WinRM connection and the
required secret references, then start the onboarding workflow. The worker
deploys the registered agent workflow. Once the agent checks in, the controller
can run its health check and collect package inventory. Target onboarding is
separate from creating an LXC guest: the guest or server must already exist and
be reachable.

Linux package inventories include the installed version and, when APT reports
an available upgrade after refreshing repository metadata, its candidate
version. Connected LXC targets can also run read-only Docker discovery through
the agent over HTTP on the private network (TCP port 8090); the controller only
accepts resolved private-network addresses for this request.

The agent is configured with `LXCUP_AGENT_TOKEN`, `LXCUP_TARGET_ID`, and
`LXCUP_CONTROLLER_URL`. It reports heartbeats and telemetry to the controller.
The controller exposes these operational endpoints:

```text
GET  /health/live
GET  /health/ready
GET  /metrics
POST /api/v1/agents/heartbeat
GET  /api/v1/targets
```

See [`deploy/README.md`](deploy/README.md) for deployment guidance and the
project documentation for workflow and security details.

## Development and tests

Run the Rust checks:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Run frontend checks from `frontend/`:

```powershell
npm ci
npm test -- --run
npm run build
```

The repository's combined local verification script is:

```powershell
.\scripts\verify.ps1
```

PostgreSQL integration tests use only the separate test database configured by
`DATABASE_TEST_URL`; they must never point at a production database. For the
Compose test database, configure the URL using the test credentials from `.env`:

```powershell
$env:DATABASE_TEST_URL = "postgres://lxcup_test:<password>@localhost:5434/lxcup_test"
cargo test -p lxcup-persistence --test postgres_integration -- --nocapture
```

Without `DATABASE_TEST_URL`, PostgreSQL integration tests are skipped. A passing
test command in that case does not verify database-backed behavior.

An optional SonarQube analysis can be run with `sonar.ps1`. Rust and frontend
coverage tools must be installed to include their respective coverage reports.

## Safety model

Mutating update workflows follow this sequence:

```text
SCAN → PLAN → CONFIRMATION → APPLY
```

Only registered workflows and validated inputs are accepted. Review the plan
before confirming changes, and use a test target before managing production
systems.

## License

See the repository's license file, if present.
