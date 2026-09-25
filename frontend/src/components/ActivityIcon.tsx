export type ActivityIconName = "workflow" | "deploy" | "health" | "inventory" | "packages" | "configure" | "lxc" | "server" | "task" | "log" | "error";

const operationIcons: Record<string, ActivityIconName> = {
  deploy_agent: "deploy",
  update_agent: "deploy",
  repair_agent: "health",
  health_check: "health",
  collect_package_inventory: "inventory",
  update_packages: "packages",
  configure_target: "configure",
  "Agent installieren": "deploy",
  "Agent aktualisieren": "deploy",
  "Agent reparieren": "health",
  Healthcheck: "health",
  "Paketinventar sammeln": "inventory",
  "Pakete aktualisieren": "packages",
  "Ziel konfigurieren": "configure",
};

const statusTones: Record<string, string> = {
  queued: "border-[var(--primary)]/40 bg-[var(--primary-soft)] text-lxcup-primary",
  checking: "border-[var(--primary)]/40 bg-[var(--primary-soft)] text-lxcup-primary",
  planned: "border-[var(--warning)]/40 bg-[var(--warning-soft)] text-[var(--warning)]",
  applying: "border-[var(--primary)]/40 bg-[var(--primary-soft)] text-lxcup-primary",
  succeeded: "border-[var(--success)]/40 bg-[var(--success-soft)] text-[var(--success)]",
  completed: "border-[var(--success)]/40 bg-[var(--success-soft)] text-[var(--success)]",
  failed: "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]",
  aborted: "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]",
  cancelled: "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]",
  reconcile_required: "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]",
};

export function activityIconForOperation(operation: string | undefined): ActivityIconName {
  return operationIcons[operation ?? ""] ?? "workflow";
}

export function activityStatusTone(status: string): string {
  return statusTones[status] ?? "border-[var(--line)] bg-[var(--paper-muted)] text-[var(--muted)]";
}

export function ActivityIcon({ name, className = "h-[18px] w-[18px]" }: Readonly<{ name: ActivityIconName; className?: string }>) {
  const shapes: Record<ActivityIconName, React.ReactNode> = {
    workflow: <><circle cx="5" cy="5" r="2" /><circle cx="15" cy="15" r="2" /><path d="M7 5h4a3 3 0 0 1 3 3v4m-2 0 2 2 2-2M5 7v5a3 3 0 0 0 3 3h5" /></>,
    deploy: <><path d="M10 2.5v8m-3-3 3 3 3-3" /><path d="M4 12v4a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 16 16v-4" /></>,
    health: <><path d="M10 2.5 16.5 5v4.5c0 4-2.7 6.7-6.5 9-3.8-2.3-6.5-5-6.5-9V5L10 2.5Z" /><path d="M4.5 10h3l1.4-2.5 2.2 5 1.2-2.5h3.2" /></>,
    inventory: <><path d="m10 2.5 6.5 3.7v7.6L10 17.5l-6.5-3.7V6.2L10 2.5Z" /><path d="m3.7 6.4 6.3 3.7 6.3-3.7M10 10.1v7" /></>,
    packages: <><path d="M3.5 6 10 2.5 16.5 6 10 9.6 3.5 6Z" /><path d="M3.5 6v8L10 17.5l6.5-3.5V6M10 9.6v7.9" /><path d="M7 4.2 13.5 7.8" /></>,
    configure: <><circle cx="10" cy="10" r="2.4" /><path d="M10 2.5v2m0 11v2m7.5-7.5h-2m-11 0h-2m12.8-5.3-1.4 1.4m-7.8 7.8-1.4 1.4m10.6 0-1.4-1.4m-7.8-7.8L5.7 4.7" /><circle cx="10" cy="10" r="6.5" /></>,
    lxc: <><path d="m10 2.7 6.5 3.7v7.2L10 17.3l-6.5-3.7V6.4L10 2.7Z" /><path d="m3.8 6.5 6.2 3.6 6.2-3.6M10 10.1v6.7" /></>,
    server: <><rect x="3" y="3" width="14" height="5.5" rx="1" /><rect x="3" y="11.5" width="14" height="5.5" rx="1" /><path d="M6 5.75h.01M9 5.75h5M6 14.25h.01M9 14.25h5" /></>,
    task: <><rect x="4" y="3" width="12" height="14" rx="1.5" /><path d="M7 7h6m-6 3h1.5m2 0H13m-6 3h6" /><path d="m7 10 1 1 1.5-2" /></>,
    log: <><path d="m6 5 4 5-4 5m6 0h4" /><rect x="2.5" y="2.5" width="15" height="15" rx="2" /></>,
    error: <><path d="m10 3 7 13H3L10 3Z" /><path d="M10 8v3.5m0 2.5h.01" /></>,
  };
  return <svg className={className} viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{shapes[name]}</svg>;
}
