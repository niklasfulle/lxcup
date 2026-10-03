# lxcup

lxcup is a self-hosted control plane for managing updates and operational
workflows across Linux servers, LXC guests, Docker workloads, and Windows
systems. It combines a web dashboard, a Rust controller and agents, PostgreSQL,
and a restricted Ansible worker.

> **Project status:** lxcup is under active development. Core onboarding,
> workflow reconciliation, Compose-managed image updates, operational alerts,
> encrypted backups, and diagnostic exports are implemented. Before relying on
> the system in production, follow the deployment and recovery runbooks and
> verify a restore using an isolated database.

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

Admins can create backups in **System → Backups**. Each backup contains a
PostgreSQL dump, the already encrypted secret store, and versioned deployment
templates. The application encrypts the archive with the passphrase entered in
the dialog using the standard [age](https://age-encryption.org/) passphrase
format; the passphrase is never stored by lxcup. Keep it separately: losing it
makes that archive unrecoverable. The live `.env` and
`LXCUP_SECRET_MASTER_KEY` are not included, so keep the master key in a separate
secret manager or other access-controlled recovery location.

The encrypted archive remains in the persistent backup volume according to
`LXCUP_BACKUP_RETENTION_DAYS` (default 30 days) until downloaded. You can also
schedule **Backup erstellen** from **Zeitpläne** by selecting an active,
global `backup_passphrase` Secret. The secret is resolved at run time and is
not exposed in schedule responses or activity events. Scheduled backups are
retained in the same list for manual download. Copy downloaded archives
off-host; the application volume is not an offsite recovery strategy.

To inspect an archive locally, decrypt it with age; age prompts for the
passphrase:

```powershell
age --decrypt --output .\backup.tar .\lxcup-backup-<id>.tar.age
tar -xf .\backup.tar -C .\restore-test
```

To validate restore end-to-end, install `age`, PostgreSQL client tools
(`pg_restore` and `psql`), and the Rust toolchain. Set `DATABASE_TEST_URL` to a
disposable database whose name includes `test` or `restore`, provide the
externally managed `LXCUP_SECRET_MASTER_KEY`, then run:

```powershell
.\scripts\restore-backup-test.ps1 `
  -BackupFile ".\lxcup-backup-<id>.tar.age" `
  -ConfirmIsolatedDatabase
```

This test decrypts the archive (prompting for the backup passphrase), restores
into only the explicitly supplied test database, applies migrations, and
validates account/resource/inventory references, audit data, and secret-store
readability using the external master key. It is destructive to that database:
never point it at development or production data. See
[`docs/secret-store-recovery.md`](docs/secret-store-recovery.md) for recovery
details and the optional standalone Compose backup procedure.

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
