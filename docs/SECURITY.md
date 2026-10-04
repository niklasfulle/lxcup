# Security

This document records the security rules implemented by lxcup. It complements
the controls in code and deployment configuration; it is not a substitute for
them.

## Security model and scope

lxcup is a self-hosted control plane for existing Linux servers, LXC guests,
Windows systems, and Docker workloads reported by managed agents. It does not
create virtual machines or LXC guests and does not use a Proxmox control-plane
API. Treat the controller, PostgreSQL, encrypted secret store, worker, artifact
service, browser, and managed hosts as separate trust boundaries.

The worker is intentionally limited to registered operations and playbooks.
It is not a general remote shell. The worker claims jobs from the database,
enforces target-level job exclusivity, resolves only secret references attached
to the target, and builds a temporary Ansible inventory and variables file.

## Authentication and authorization

- Production must run with `LXCUP_ENV=production`, PostgreSQL, and the
  account-authentication flow enabled. The first database start creates the
  `admin` account with temporary password `admin` and requires changing it
  before any other application endpoint can be used. Change it immediately;
  subsequent starts never reset an existing account. There is no public
  registration or email-based account recovery.
- With PostgreSQL/account authentication enabled, legacy role bearer tokens
  are not accepted for human API routes. They do not create or map a user and
  cannot silently gain Admin privileges. Agent heartbeat credentials remain a
  separate per-target mechanism.
- Human account sessions are random, expire after eight hours, and are stored
  only as hashes. The raw session secret is issued only in an `HttpOnly`,
  `SameSite=Strict` cookie scoped to `/api/v1`; production also sets `Secure`.
  It is never returned in JSON or exposed to frontend JavaScript, so a page
  reload can restore the session without putting the credential in web storage.
  A separate `SameSite=Strict` CSRF cookie is echoed in `X-CSRF-Token` for
  state-changing cookie-authenticated requests; the controller compares both
  values before invoking a handler. Legacy explicit bearer-token auth remains
  supported where account auth is disabled. Passwords use salted
  PBKDF2-HMAC-SHA256. Usernames and passwords are never returned by management
  endpoints; password values are omitted from audit metadata.
- The supported account roles are Admin and User. Admin-only user management,
  secrets, and user audit APIs are checked server-side, not only hidden in the
  frontend. User accounts retain ordinary resource/workflow permissions but
  cannot access those administrative endpoints. The last active Admin cannot
  be demoted, disabled, or deleted.
- User activity records include the actor, role, action, resource, status, and
  request identifier. Login success/failure, logout, password changes, user
  management, and resource mutations receive stable action identifiers. Audit
  metadata omits request bodies, passwords, bearer values, secret values, and
  other credential material. Authenticated write requests first persist a
  pending audit record before their handler runs, then update its result code;
  a process interruption therefore leaves a visible pending record instead of
  silently losing the attempted action. If the reservation cannot be stored,
  the write handler is not run. Only Admins can read the user audit API.
- The server authorizes every protected request. Frontend visibility is not an
  authorization boundary. Administrative secret lifecycle operations and
  destructive actions require the corresponding server-side role. Update
  policy deletion requires destructive permission and explicit confirmation;
  the system-wide standard policy cannot be deleted.
- Use TLS for browser access in production. Do not expose session or bearer
  tokens over plaintext HTTP or place them in URLs.
- Removing a registered target is an Admin-only destructive action that
  requires explicit confirmation and is rejected while target workflows are
  active. It removes controller-side target data but does not uninstall the
  agent or alter the host. Secret records are retained because credentials
  may be shared; only their reference from the deleted target is removed.
- Never persist account-session credentials in Local Storage or session
  storage. The account session is held by the HttpOnly cookie; only the
  non-authenticating CSRF value is readable by the frontend.
- Agent heartbeats use the target's agent token, independently of user-session
  tokens. Rotate or revoke credentials using the supported secret lifecycle.
- Windows update workflows are outbound agent claims only. The controller
  authenticates each claim/result with the target's agent token and permits only
  allowlisted workflow actions. Update installs use validated exact winget IDs
  locally; Windows package changes must never fall back to WinRM or caller-built
  commands.
- The Windows setup script verifies the versioned manifest, SHA-256, PE32+
  format, and amd64 machine type before replacing the local executable. It
  restricts the ProgramData configuration directory and token file to SYSTEM
  and local Administrators, and does not place the token in process arguments.
  The service runs as LocalSystem to support system-wide winget installation;
  therefore each approved update has high local privilege. Keep Windows update
  policies narrow and perform the documented isolated-host acceptance before
  production use.

For the implemented role behavior and production requirements, see
[`operations.md`](operations.md) and [`../deploy/README.md`](../deploy/README.md).

## Secrets and configuration

- Never commit credentials, private keys, agent tokens, production connection
  strings, or secret-store contents. `.env` is local-only; `.env.example` may
  contain variable names and clearly fake placeholders only.
- Secret metadata and secret values are separate. PostgreSQL holds references
  and metadata; values are stored by the encrypted file-backed secret store.
  Protect `LXCUP_SECRET_MASTER_KEY` as a high-value key and keep recovery
  material separate from encrypted backups.
- Do not return secret values from list/read APIs or expose them to the browser
  after a write. Pass references across internal boundaries, not secret
  material, except at the component that must use it.
- Worker and agent logs must redact credentials and tokens. Never include
  secret values in job events, errors, metrics, audit metadata, or test output.
- Prefer explicit allowlists of environment variables, secret kinds, and
  workflow parameters over pass-through configuration.
- The Admin support-diagnostics export is locally initiated and bounded. It
  includes resource addresses and operational status, but excludes secret
  material, raw worker stdout/stderr, process environments, and private keys;
  only allowlisted workflow summaries are exported. The central filter drops
  credential-labelled lines, URL user-info, common provider token formats,
  JWTs, and complete PEM blocks before bounding each summary. Treat downloaded
  bundles as operationally sensitive and share them only through an approved
  channel.
- Admins can create encrypted database and secret-store backups in the
  application. The controller coordinates mutating HTTP requests while taking
  the cross-store snapshot, encrypts it in the standard age passphrase format
  with a password entered for that backup, and retains only the encrypted
  archive in a dedicated persistent backup volume. The password is not stored;
  losing it makes that backup unrecoverable. `LXCUP_SECRET_MASTER_KEY` and the
  live `.env` are excluded; Compose files and `.env.example` are bundled only
  as versioned templates. Keep both the backup password and master key
  protected and copy downloads off-host; the local volume alone is not a
  disaster recovery strategy.
- A recurring backup schedule may reference an active, global
  `backup_passphrase` Secret. PostgreSQL stores only the Secret ID alongside
  the schedule; neither the Secret value nor that reference is returned in
  schedule responses or activity events. The controller resolves the current
  value from the encrypted Secret Store only when starting the backup. Rotating
  or revoking the Secret changes or disables future runs; older archives still
  require the passphrase that encrypted them.

## Inputs, database, and errors

- Validate untrusted data on the server even when the UI already validates it.
  Use the domain types and validation rules in `lxcup-core` rather than
  reimplementing weaker checks in handlers.
- Use SQLx parameter binding and repository methods. Do not build SQL by
  concatenating user-controlled values.
- Apply authorization before reading or mutating protected resources. Keep
  target identity, secret references, operation, mode, and parameters bound and
  validated server-side.
- Return actionable but sanitized errors. Do not send stack traces, SQL details,
  internal filesystem paths, credentials, or process environments to clients.
- Correlate server and worker diagnostics using request/job/target identifiers;
  identifiers are useful context but must never be treated as credentials.

## Workflow and supply-chain safety

- Only registered operations, playbooks, and fixed parameter shapes may run.
  Never introduce arbitrary command strings, user-supplied playbook paths, or
  unrestricted Ansible extra-vars.
- Mutating package operations follow scan → plan → explicit confirmation →
  apply. Preserve idempotency, target exclusivity, audit events, and
  reconciliation behavior. A package selection of `*` means all installed
  packages with available updates; it is accepted only under a policy with no
  package-name allowlist and remains subject to the policy's target, risk, and
  maintenance-window restrictions. Apply still requires the matching
  successful plan and explicit confirmation.
- Agent deployment/update artifacts require an approved manifest, matching
  version, and validated SHA-256 plus platform executable format/architecture
  before execution. Do not bypass verification to make a deployment succeed.
- SSH host keys must be verified using the registered known-hosts secret. Do
  not disable host-key checking as a general workaround.
- Keep the worker least-privileged and its secret-store mount read-only. Avoid
  expanding network exposure or container privileges without an explicit
  security review.
- The Linux agent joins the `docker` group only when that group exists, so it
  can inspect the local Docker daemon. This access is effectively root-level on
  the managed host; operators must understand the consequence before running
  the host preparation script or deploying the agent on Docker hosts.
  The standalone host preparation script runs directly as root or re-executes
  its saved file through sudo. Without root or sudo it exits before making
  changes. It prints local public SSH host keys in known-hosts format; no
  additional helper script is required. Register the resource using a hostname
  or address present in that output (custom aliases require a matching entry).
- Docker image updates are destructive and require explicit confirmation plus
  the destructive/admin permission. Only a Compose-managed Linux service with
  one replica and a single in-project config file may be recreated. The agent
  checks the saved Compose service hash and expected remote image digest before
  pulling, then recreates only the selected service without dependencies.
  A Docker-group agent is effectively root on its host; do not expand this
  capability to arbitrary containers or caller-provided Compose paths.
  There is no automatic rollback; see the operator recovery procedure in
  [`docker-image-update-recovery.md`](docker-image-update-recovery.md).

## Changes and verification

For a security-sensitive change, review the relevant API handler, domain rule,
worker/agent boundary, and tests—not only the UI. Run focused tests and the
applicable quality checks. SonarQube scans are run manually by the project
owner at the end of a task; agents must not initiate scans. Never weaken
authentication, validation, TLS, secret handling, or artifact verification
solely to make a test pass; use isolated test fixtures instead.
