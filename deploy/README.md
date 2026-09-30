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
5. Schedule `scripts/backup-postgres.ps1` on the PostgreSQL LXC and regularly
   test restore into a separate test database.
6. Monitor `/health/live`, `/health/ready` and `/metrics`; logs are structured
   JSON and must be forwarded by the host's normal log collector.

There is intentionally no GitHub Actions workflow. Formatting, tests, Clippy,
frontend build and optional SonarQube checks remain local or operator-driven
quality gates.
