## Secrets scanning

Do not run SonarQube scans, including the secrets scanner. Niklas runs all
SonarQube scans manually at the end of a task. Do not make a scan a prerequisite
for reading files or block implementation on one; leave the final scan to the
user and report the other verification performed.

## External services

- GitHub is reachable outside the filesystem sandbox through the configured CI
  integration. If an in-sandbox GitHub connector or CLI is unavailable, use
  that CI route for authorized GitHub operations.
- SonarQube is also available outside the filesystem sandbox. Request
  escalation when a scan or query needs access beyond the sandbox.

## Project documentation

Before changing code, read the relevant project guide in `docs/`:

- Security-sensitive work: `docs/SECURITY.md`.
- Rust, React, CSS, or test changes: `docs/CODE_STYLE.md` and, for UI work,
  `docs/DESIGN_SYSTEM.md`.
- Persistence or schema changes: `docs/DATABASE.md`.
- HTTP, events, or agent protocol changes: `docs/API.md`.
- Cross-component or deployment changes: `docs/ARCHITECTURE.md`.

`docs/README.md` is the documentation index. Existing operational procedures,
quality gates, secret recovery, and worker setup remain in their focused
runbooks. Keep documentation aligned with implementation and tests; do not
copy generic examples or document routes, variables, or guarantees that the
code does not provide. If documentation conflicts with the implementation,
call out the discrepancy and verify the source of truth before making a broad
change.

## Implementation conventions

- Preserve the self-hosted, agent-first architecture. lxcup manages existing
  resources; it does not create LXC guests or depend on a Proxmox control-plane
  API.
- Keep files below 800 lines where practical. Split cohesive responsibilities
  into focused modules before a file grows beyond that size.
- For frontend UI, keep Tailwind utility classes on the elements they style;
  do not move the design system into a centralized `ui.ts` class catalogue.
- Run the narrowest relevant tests, then the applicable build/checks described
  in `docs/CODE_STYLE.md` and `docs/quality-gates.md`.
