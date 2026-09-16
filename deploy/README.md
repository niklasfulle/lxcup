# Production deployment

The management application is intended to run in its own management LXC. The
production PostgreSQL instance remains the existing PostgreSQL service in its
dedicated LXC; `DATABASE_URL` is injected by the service manager and is never
committed to the repository.

## Checklist

1. Build the server image from `deploy/compose.prod.yaml`.
2. Set `DATABASE_URL`, `LXCUP_AUTH_*_TOKEN` and `LXCUP_AUTH_REQUIRED=true` in
   the management LXC secret store.
3. Put Caddy or another reverse proxy in front of port 8080 and terminate TLS.
4. Allow only the management LXC to reach the agent ports.
5. Schedule `scripts/backup-postgres.ps1` on the PostgreSQL LXC and regularly
   test restore into a separate test database.
6. Monitor `/health/live`, `/health/ready` and `/metrics`; logs are structured
   JSON and must be forwarded by the host's normal log collector.

There is intentionally no GitHub Actions workflow. Formatting, tests, Clippy,
frontend build and optional SonarQube checks remain local or operator-driven
quality gates.
