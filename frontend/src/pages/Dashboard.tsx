import { Link } from "react-router-dom";
import type { AnsibleJobDto, TargetDto } from "../api";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { useAnsibleJobs, useTargets, useWorkerAvailability } from "../queries";
import { cn } from "../classnames";

const activeStatuses = new Set(["queued", "checking", "planned", "applying", "reconcile_required"]);

export function Dashboard() {
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const worker = useWorkerAvailability();
  const targetList = targets.data ?? [];
  const jobList = jobs.data ?? [];
  const connectedCount = targetList.filter((target) => target.state === "managed").length;
  const pendingCount = targetList.filter((target) => target.state === "pending").length;
  const activeCount = jobList.filter((job) => activeStatuses.has(job.status)).length;
  const recentJobs = [...jobList].sort((left, right) => Date.parse(right.updated_at) - Date.parse(left.updated_at)).slice(0, 6);
  const workerLabel = workerStatusLabel(worker.isLoading, worker.data?.available);
  const workerTone = workerStatusTone(worker.isLoading, worker.data?.available);

  return <div className="grid gap-5">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4 max-[720px]:items-start">
      <div>
        <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Datacenter / lxcup-control</p>
        <h1 className="mb-1">Übersicht</h1>
        <p className="mb-0 text-sm text-[var(--muted)]">Systemzustand, Ressourcen und aktuelle Automatisierung auf einen Blick.</p>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <output className={cn("inline-flex min-h-9 items-center gap-2 border px-3 py-2 text-xs font-semibold", workerTone)}><span className="h-2 w-2 rounded-full bg-current" aria-hidden="true" />Worker · {workerLabel}</output>
        <Link className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to="/workflows">Workflows öffnen <span className="ml-2" aria-hidden="true">→</span></Link>
      </div>
    </header>

    <section className="grid grid-cols-2 gap-3 xl:grid-cols-4" aria-label="Systemkennzahlen">
      <MetricCard label="Ressourcen" value={targets.isLoading ? "…" : targetList.length} note="registrierte Ziele" tone="primary" />
      <MetricCard label="Agenten verbunden" value={targets.isLoading ? "…" : connectedCount} note={targetList.length ? `von ${targetList.length} Ressourcen` : "noch keine Agenten"} tone="success" />
      <MetricCard label="Onboarding offen" value={targets.isLoading ? "…" : pendingCount} note={pendingCount ? "warten auf Agent oder Heartbeat" : "keine offenen Onboardings"} tone={pendingCount ? "warning" : "neutral"} />
      <MetricCard label="Aktive Jobs" value={jobs.isLoading ? "…" : activeCount} note={jobs.error ? "Jobstatus nicht verfügbar" : `${jobList.filter((job) => job.status === "failed").length} fehlgeschlagen`} tone={activeCount ? "primary" : "neutral"} />
    </section>

    <section className="grid items-start gap-4 xl:grid-cols-[minmax(0,1.1fr)_minmax(20rem,0.9fr)]" aria-label="Ressourcen und Aktivität">
      <ResourceInventory targets={targets} />
      <RecentJobs jobs={jobs} recentJobs={recentJobs} targets={targetList} />
    </section>

    <section aria-labelledby="resource-types-title">
      <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
        <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Ressourcenbereiche</p><h2 id="resource-types-title" className="mb-0">Infrastruktur verwalten</h2></div>
        <Link className="text-xs font-semibold text-lxcup-primary hover:underline" to="/targets">Alle Ziele →</Link>
      </div>
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <ResourceTypeCard title="Linux-Server" count={countKind(targetList, "linux_server")} to="/servers" description="SSH-Ziele verwalten" />
        <ResourceTypeCard title="LXC-Container" count={countKind(targetList, "lxc")} to="/containers" description="Manuell eingebundene LXCs" />
        <ResourceTypeCard title="Windows" count={countKind(targetList, "windows_server")} to="/windows" description="WinRM-Ziele verwalten" />
        <ResourceTypeCard title="Docker-Container" countLabel="Agent Discovery" to="/docker" description="Container auf LXC-Agenten" />
      </div>
    </section>
  </div>;
}

function MetricCard({ label, value, note, tone }: Readonly<{ label: string; value: string | number; note: string; tone: "primary" | "success" | "warning" | "neutral" }>) {
  const valueClass = {
    primary: "text-lxcup-primary",
    success: "text-[var(--success)]",
    warning: "text-[var(--warning)]",
    neutral: "text-[var(--ink)]",
  }[tone];
  return <article className="grid min-h-28 content-between gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
    <span className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">{label}</span>
    <div className="flex items-end justify-between gap-2"><strong className={cn("text-3xl font-semibold leading-none", valueClass)}>{value}</strong><span className="text-right text-[11px] leading-snug text-[var(--muted)]">{note}</span></div>
  </article>;
}

function workerStatusLabel(isLoading: boolean, available: boolean | undefined) {
  if (isLoading) return "Prüft…";
  return available ? "Verfügbar" : "Nicht verfügbar";
}

function workerStatusTone(isLoading: boolean, available: boolean | undefined) {
  if (isLoading) return "border-[var(--line)] bg-[var(--panel)] text-[var(--muted)]";
  return available ? "border-[var(--success)] bg-[var(--success-soft)] text-[var(--success)]" : "border-[var(--error)] bg-[var(--error-soft)] text-[var(--error)]";
}

function ResourceInventory({ targets }: Readonly<{ targets: ReturnType<typeof useTargets> }>) {
  const items = targets.data ?? [];
  return <section className="border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-labelledby="dashboard-targets-title">
    <div className="mb-3 flex items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 id="dashboard-targets-title" className="mb-1">Ressourcen</h2><p className="mb-0 text-xs text-[var(--muted)]">Agentstatus und Versionsstand der verwalteten Ziele.</p></div><span className="text-xs text-[var(--muted)]">{targets.isLoading ? "…" : `${items.length} Ziele`}</span></div>
    {resourceInventoryContent(targets, items)}
  </section>;
}

function resourceInventoryContent(targets: ReturnType<typeof useTargets>, items: TargetDto[]) {
  if (targets.isLoading) return <EmptyMessage>Ressourcen werden geladen…</EmptyMessage>;
  if (targets.error) return <p className="m-0 border border-[var(--error)] bg-[var(--error-soft)] p-3 text-sm font-semibold text-[var(--error)]" role="alert">Ressourcen konnten nicht geladen werden: {targets.error.message}</p>;
  if (items.length === 0) return <EmptyMessage>Es sind noch keine Ressourcen registriert. Wähle unten den passenden Ressourcenbereich aus.</EmptyMessage>;
  return <ul className="m-0 grid list-none gap-2 p-0">{items.map((target) => <TargetRow key={target.id} target={target} />)}</ul>;
}

function TargetRow({ target }: Readonly<{ target: TargetDto }>) {
  const state = targetState(target.state);
  return <li><Link className="flex flex-wrap items-center justify-between gap-3 border border-[var(--line)] bg-[var(--paper)] px-3 py-3 transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" to={`/targets/${target.id}`}>
    <span className="min-w-0"><strong className="block truncate text-sm text-[var(--ink)]">{target.name}</strong><span className="mt-1 block text-xs text-[var(--muted)]">{kindLabel(target.kind)} · {target.address} · {target.transport.toUpperCase()}</span></span>
    <span className="flex shrink-0 items-center gap-2"><span className={cn("inline-flex items-center px-2 py-1 text-[10px] font-bold", state.className)}>{state.label}</span><span className="hidden text-xs text-[var(--muted)] sm:inline">{agentVersionLabel(target)}</span><span className="text-lxcup-primary" aria-hidden="true">→</span></span>
  </Link></li>;
}

function agentVersionLabel(target: TargetDto) {
  if (target.agent_version) return `Agent ${target.agent_version}`;
  if (target.state === "managed") return "Version unbekannt";
  return "Agent fehlt";
}

function RecentJobs({ jobs, recentJobs, targets }: Readonly<{ jobs: ReturnType<typeof useAnsibleJobs>; recentJobs: AnsibleJobDto[]; targets: TargetDto[] }>) {
  return <section className="border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-labelledby="dashboard-jobs-title">
    <div className="mb-3 flex items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 id="dashboard-jobs-title" className="mb-1">Letzte Workflows</h2><p className="mb-0 text-xs text-[var(--muted)]">Zuletzt aktualisierte Jobausführungen.</p></div><Link className="text-xs font-semibold text-lxcup-primary hover:underline" to="/workflows">Alle öffnen →</Link></div>
    {recentJobContent(jobs, recentJobs, targets)}
  </section>;
}

function recentJobContent(jobs: ReturnType<typeof useAnsibleJobs>, recentJobs: AnsibleJobDto[], targets: TargetDto[]) {
  if (jobs.isLoading) return <EmptyMessage>Workflows werden geladen…</EmptyMessage>;
  if (jobs.error) return <p className="m-0 border border-[var(--error)] bg-[var(--error-soft)] p-3 text-sm font-semibold text-[var(--error)]" role="alert">Workflows konnten nicht geladen werden: {jobs.error.message}</p>;
  if (recentJobs.length === 0) return <EmptyMessage>Noch keine Workflows ausgeführt. Starte einen Healthcheck oder Inventar-Job.</EmptyMessage>;
  return <ul className="m-0 grid list-none gap-2 p-0">{recentJobs.map((job) => <li key={job.id}><Link className="flex items-center justify-between gap-3 border border-[var(--line)] bg-[var(--paper)] px-3 py-3 transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" to={`/workflows/${job.id}`}>
    <span className="min-w-0"><strong className="block truncate text-sm">{operationLabel(job.operation)}</strong><span className="mt-1 block truncate text-xs text-[var(--muted)]">{jobTargetName(job, targets)} · {new Date(job.updated_at).toLocaleString()}</span></span>
    <span className={cn("shrink-0 px-2 py-1 text-[10px] font-bold", jobStatusBadgeClass(job.status))}>{jobStatusLabel(job.status)}</span>
  </Link></li>)}</ul>;
}

function ResourceTypeCard({ title, count, countLabel, to, description }: Readonly<{ title: string; count?: number; countLabel?: string; to: string; description: string }>) {
  return <Link className="group flex min-h-24 flex-col justify-between gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" to={to}>
    <span className="flex items-center justify-between gap-2"><strong className="text-sm">{title}</strong><span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2 py-1 text-[10px] font-bold text-[var(--muted)]">{countLabel ?? `${count ?? 0} Ziele`}</span></span>
    <span className="flex items-end justify-between gap-2 text-xs text-[var(--muted)]"><span>{description}</span><span className="font-semibold text-lxcup-primary group-hover:translate-x-0.5" aria-hidden="true">→</span></span>
  </Link>;
}

function EmptyMessage({ children }: Readonly<{ children: React.ReactNode }>) {
  return <p className="m-0 grid min-h-32 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] p-4 text-center text-sm text-[var(--muted)]">{children}</p>;
}

function countKind(targets: TargetDto[], kind: TargetDto["kind"]) {
  return targets.filter((target) => target.kind === kind).length;
}

function kindLabel(kind: TargetDto["kind"]) {
  const labels = { lxc: "LXC-Container", linux_server: "Linux-Server", windows_server: "Windows" };
  return labels[kind];
}

function targetState(state: TargetDto["state"]) {
  const labels = {
    managed: { label: "Verbunden", className: "bg-[var(--success-soft)] text-[var(--success)]" },
    pending: { label: "Onboarding", className: "bg-[var(--warning-soft)] text-[var(--warning)]" },
    disabled: { label: "Deaktiviert", className: "bg-[var(--paper-muted)] text-[var(--muted)]" },
  };
  return labels[state];
}

function operationLabel(operation: string) {
  const labels: Record<string, string> = {
    deploy_agent: "Agent installieren",
    update_agent: "Agent aktualisieren",
    repair_agent: "Agent reparieren",
    health_check: "Healthcheck",
    collect_package_inventory: "Paketinventar erfassen",
    update_packages: "Pakete aktualisieren",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}

function jobTargetName(job: AnsibleJobDto, targets: TargetDto[]) {
  if ("target" in job.target) {
    const targetId = job.target.target;
    return targets.find((target) => target.id === targetId)?.name ?? "Ziel nicht gefunden";
  }
  if ("container" in job.target) return `Container ${job.target.container}`;
  return `Node ${job.target.node}`;
}
