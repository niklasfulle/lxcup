# Production deployment

The management application is intended to run in its own management LXC. The
production PostgreSQL instance remains the existing PostgreSQL service in its
dedicated LXC; `DATABASE_URL` is injected by the service manager and is never
committed to the repository.

## Checklist

1. Build the controller and production frontend images from
   `deploy/compose.prod.yaml`. Caddy serves the frontend over HTTPS; the
   frontend forwards `/api/*` to the controller over the private Compose
   network.
2. Set `DATABASE_URL` in the management LXC secret store. The first server
   start creates the initial `admin` / `admin` account in PostgreSQL and
   requires a password change at first login; change it immediately.
3. Keep the controller bound to loopback/private networking and terminate TLS
   at Caddy. Do not expose the frontend container or controller port directly
   to the public network.
4. Allow only the management LXC to reach the agent ports.
5. In **System → Backups**, create a passphrase-encrypted backup and download it
   to an access-controlled off-host location. Configure `LXCUP_BACKUP_DIR` as
   persistent storage with enough free space and set an appropriate
   `LXCUP_BACKUP_RETENTION_DAYS` value. For unattended backups, create a
   **Backup erstellen** schedule using an active global `backup_passphrase`
   Secret. Regularly run the documented restore test against a separate,
   disposable database; preserve `LXCUP_SECRET_MASTER_KEY` outside the backup.
6. Monitor `/health/live`, `/health/ready` and `/metrics`; logs are structured
   JSON and must be forwarded by the host's normal log collector.

There is intentionally no GitHub Actions workflow. Formatting, tests, Clippy,
frontend build and optional SonarQube checks remain local or operator-driven
quality gates.

The in-application backup archive is stored on the dedicated persistent backup
volume until manually downloaded. Scheduled runs do not upload or send the
archive to an external destination. Operators are responsible for downloading
off-host copies and periodically validating recovery; see
[`../docs/secret-store-recovery.md`](../docs/secret-store-recovery.md).
