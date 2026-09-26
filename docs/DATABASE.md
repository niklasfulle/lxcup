# Database

PostgreSQL is the controller's durable source of truth. SQLx repositories
provide access to it; schema changes are versioned SQL files embedded in
`crates/lxcup-persistence/migrations/`.

## Connection and environments

- The server and worker use `DATABASE_URL` supplied by the environment or
  Compose. Never commit a real connection string.
- Database integration tests require the explicit `DATABASE_TEST_URL`. It must
  identify a disposable test/restore database; tests must not guess a fallback
  URL or connect to a development/production database.
- Keep development, test/restore, and production databases separate. For backup
  and restore procedures, see [`operations.md`](operations.md) and
  [`secret-store-recovery.md`](secret-store-recovery.md).

## Persistence model

The current schema is represented by the ordered SQL migrations. At a domain
level it stores:

- **Targets** — registered existing Linux, LXC, or Windows resources, their
  connection metadata, lifecycle, and secret references. Secret values are
  not stored in PostgreSQL.
- **Ansible jobs and job events** — registered operation, validated parameters,
  target, status, timestamps, idempotency information, and the ordered
  execution log used by the UI and audit trail.
- **Agent registrations and worker heartbeats** — managed-agent enrollment and
  worker availability signals.
- **Package inventory and target telemetry** — latest package snapshot and
  resource samples associated with a target.
- **Docker workloads and discovery runs** — discovered workloads and their
  management/discovery state.
- **Schedules and update policies** — recurring registered work and package
  update constraints. Non-system update policies can be deleted through the
  repository; job/event history remains independent of the policy row.
- **Planner/execution and audit records** — scan results, plans, confirmed
  executions, results, and lifecycle/audit events used by the existing planner
  domain.

Some earlier migrations preserve planner-era `nodes` and `containers` tables;
their presence is not a Proxmox API integration. The later target model is the
control plane's manually registered resource model. Use the current domain and
repository code when determining which aggregate owns a new field.

IDs are stable UUIDs for targets, jobs, and agent registrations. Related rows
use foreign keys and indexes declared by migrations. Repository methods define
the supported query/update behavior; JSON payload columns do not justify
bypassing domain validation.

## Migration rules

1. Add a new, monotonically numbered SQL migration for every schema change.
2. Do not edit an already-applied migration to change an existing database.
3. Review foreign keys, deletion behavior, checks, indexes, and compatibility
   with both server and worker before applying the migration.
4. Update persistence repositories and their tests in the same change.
5. Run migrations and PostgreSQL integration tests only against the explicit
   isolated test database.
6. Consider rollback and backup/restore implications for destructive or
   high-volume changes. Prefer expand-and-contract when older binaries may be
   running during deployment.

Never clear, reset, or manually mutate a production database to get around a
migration failure. Use the checked-in reset utility only when its isolated
target is explicitly verified; see the root README and operational runbooks.

## Data safety

- Bind SQL parameters; never concatenate user input into SQL.
- Keep transactions around repository operations that must be atomic.
- Add database constraints for invariants that must survive concurrent writes.
- Store only data needed for control-plane operation. Do not persist credentials
  or duplicate derived facts without a clear lifecycle reason.
- Preserve audit/job-event history needed to explain changes and failures.
- Protect database backups and the encrypted secret store together, and test
  restores into a disposable database before relying on them.
