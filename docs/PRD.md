# Product requirements

## Product

lxcup is a self-hosted control plane for safely operating existing Linux
servers, manually registered LXC guests, Windows systems, and Docker workloads
reported by managed agents. It provides one auditable place to onboard systems,
inspect their state, and run approved operational workflows.

This is a concise product boundary, not a promise that every capability is
production-complete. The root README describes current implementation status;
[`../lxcup_projektplan.md`](../lxcup_projektplan.md) is the broader evolving
project plan.

## Users and needs

- **Administrator** — configure the control plane and secrets, manage access,
  and investigate system-wide health and failures.
- **Operator** — register and maintain resources, run approved workflows, and
  understand their results without handling raw infrastructure credentials.
- **Viewer** — inspect resource health, inventory, telemetry, notifications,
  and workflow history without making changes.

All user-facing permissions are enforced by the controller; hiding an action
in the UI is not an authorization mechanism.

## Product goals

- Provide a clear distinction between Linux servers, LXC resources, Windows
  systems, and Docker workloads discovered on managed hosts.
- Support controlled onboarding through secret references, registered agent
  deployment, authenticated heartbeats, and follow-up health/inventory work.
- Make operational activity explainable through persisted status, ordered
  events, complete worker output, failure codes, and notifications.
- Collect package inventory and recent resource telemetry for managed targets.
- Run recurring work and package updates only under explicit policies,
  validation, and confirmation appropriate to the operation.
- Keep the deployment self-hosted and operable through documented Compose and
  production procedures.

## Scope and non-goals

In scope: manually registering already-existing resources, SSH/WinRM-based
approved Ansible workflows, agent-based status and telemetry, package
inventory, package update planning/execution, Docker discovery on managed
hosts, audit/history, and operational scheduling.

Not in scope: provisioning or deleting virtual machines/LXC guests, a Proxmox
API integration, arbitrary remote shell access, arbitrary user-supplied
playbooks, or making the browser a trusted policy boundary.

## Product principles

- **Safety before convenience:** validate on the server, require explicit
  confirmation for mutations, and preserve target exclusivity and auditability.
- **Secrets stay references until needed:** users and APIs choose secret
  references; only the authorized component resolves the value at execution.
- **Visible operational truth:** report worker availability, status, progress,
  and diagnostic logs rather than implying that queued work is executing.
- **Consistent resource model:** each resource has one home and lifecycle;
  discovered Docker workloads remain subordinate to their managed host.
- **Incremental delivery:** document behavior that is implemented and tested;
  identify incomplete capabilities instead of presenting them as guarantees.
