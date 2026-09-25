import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { cn } from "../classnames";
import { ActivityIcon, activityIconForOperation, activityStatusTone } from "../components/ActivityIcon";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { useAnsibleJobs, useContainers, useDockerDiscovery, useDockerWorkloads, usePackageInventory, useTargetTelemetry, useTargets } from "../queries";

type Target = NonNullable<ReturnType<typeof useTargets>["data"]>[number];
type InventoryQuery = ReturnType<typeof usePackageInventory>;
type TelemetryQuery = ReturnType<typeof useTargetTelemetry>;
type DockerWorkloadsQuery = ReturnType<typeof useDockerWorkloads>;
type DockerDiscoveryQuery = ReturnType<typeof useDockerDiscovery>;
type ContainerItem = NonNullable<ReturnType<typeof useContainers>["data"]>[number];
const staleAfterMs = 2 * 60 * 1000;

function stateLabel(state: string) {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Ausstehend";
}

function targetKindLabel(kind: string) {
  if (kind === "lxc") return "LXC-Container";
  if (kind === "windows_server") return "Windows";
  return "Server";
}

function targetListPath(kind: string) {
  if (kind === "lxc") return "/containers";
  if (kind === "windows_server") return "/windows";
  return "/servers";
}

function targetStateClass(state: string) {
  if (state === "managed") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (state === "disabled") return "bg-[var(--paper-muted)] text-[var(--muted)]";
  return "bg-[var(--warning-soft)] text-[var(--warning)]";
}

export function TargetDetailPage() {
  const { targetId } = useParams();
  const [now, setNow] = useState(() => Date.now());
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const containers = useContainers();
  const inventory = usePackageInventory(targetId);
  const telemetry = useTargetTelemetry(targetId);
  const target = targets.data?.find((item) => item.id === targetId);
  const targetJobs = (jobs.data ?? []).filter((job) => "target" in job.target && job.target.target === targetId).slice(0, 8);
  const hostContainer = (containers.data ?? []).find((container) => container.name === target?.name);
  const dockerWorkloads = useDockerWorkloads(target?.kind === "lxc" ? hostContainer?.id : undefined);
  const dockerDiscovery = useDockerDiscovery(target?.kind === "lxc" ? hostContainer?.id : undefined);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 5_000);
    return () => clearInterval(timer);
  }, []);
  const samples = telemetry.data?.samples ?? [];
  const latest = samples.at(-1);
  const heartbeatAge = target ? now - new Date(target.updated_at).getTime() : 0;
  const stale = heartbeatAge > staleAfterMs;
  const inventoryStale = Boolean(inventory.data?.collected_at && Date.now() - new Date(inventory.data.collected_at).getTime() > 24 * 60 * 60 * 1000);
  const dockerStale = Boolean(dockerDiscovery.data?.finished_at && Date.now() - new Date(dockerDiscovery.data.finished_at).getTime() > 15 * 60 * 1000);

  if (targets.isLoading) return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><p className="text-[var(--muted)]">Ziel wird geladen…</p></section>;
  if (!target) return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><p className="font-semibold text-[var(--error)]">Ziel nicht gefunden.</p><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/targets">Zur Zielübersicht</Link></section>;

  return <div className="grid gap-4">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4 max-[720px]:items-start">
      <div className="min-w-0"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">{targetKindLabel(target.kind)}</p><h1 className="break-words">{target.name}</h1><div className="mt-2 flex flex-wrap items-center gap-2 text-sm text-[var(--muted)]"><span>{target.address}</span><span aria-hidden="true">·</span><span className="border border-[var(--line)] bg-[var(--panel)] px-2 py-1 text-[10px] font-bold uppercase tracking-wider">{target.transport}</span></div></div>
      <Link className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={targetListPath(target.kind)}>← Ressourcenübersicht</Link>
    </header>
    <section className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3" aria-label="Zielstatus">
      <article className="grid min-h-24 content-between gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><span className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">Lifecycle</span><strong className={cn("inline-flex w-fit items-center px-2.5 py-1 text-xs font-bold", targetStateClass(target.state))}>{stateLabel(target.state)}</strong></article>
      <article className="grid min-h-24 content-between gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><span className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">Agent-Version</span><strong className="text-lg">{target.agent_version ?? <span className="text-sm font-normal text-[var(--muted)]">Noch nicht gemeldet</span>}</strong></article>
      <article className="grid min-h-24 content-between gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><span className="text-xs font-semibold uppercase tracking-wider text-[var(--muted)]">Letzter Heartbeat</span><div><strong className="block">{new Date(target.updated_at).toLocaleString()}</strong><span className={cn("mt-2 inline-flex items-center px-2 py-1 text-xs font-bold", stale ? "bg-[var(--warning-soft)] text-[var(--warning)]" : "bg-[var(--success-soft)] text-[var(--success)]")}>{stale ? "Veraltet" : "Aktuell"}</span></div></article>
    </section>
    {stale ? <output className="flex items-start gap-3 border border-[var(--warning)] bg-[var(--warning-soft)] p-4 text-sm text-[var(--ink)]"><span className="grid h-6 w-6 shrink-0 place-items-center border border-[var(--warning)] text-xs font-bold text-[var(--warning)]" aria-hidden="true">!</span><span><strong className="block">Heartbeat veraltet</strong><span>Der letzte Heartbeat liegt mehr als 2 Minuten zurück. Telemetrie und Agentstatus können veraltet sein.</span></span></output> : null}
    <section className="grid grid-cols-1 gap-4 xl:grid-cols-2" aria-label="Ressourcenstatus">
      <TargetPackageInventory target={target} inventory={inventory} stale={inventoryStale} />
      <TargetTelemetry telemetry={telemetry} samples={samples} latest={latest} />
      <TargetDockerInventory target={target} host={hostContainer} workloads={dockerWorkloads} discovery={dockerDiscovery} stale={dockerStale} />
    </section>
    <TargetWorkflowList targetId={target.id} jobs={jobs} targetJobs={targetJobs} />
  </div>;
}

function TargetPackageInventory({ target, inventory, stale }: Readonly<{ target: Target; inventory: InventoryQuery; stale: boolean }>) {
  const summary = packageInventorySummary(inventory);
  return <article className="flex min-h-44 flex-col border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><div className="mb-3 flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 className="mb-1">Paketinventar</h2><p className="mb-0 text-sm text-[var(--muted)]">{summary}</p></div><Link className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={`/targets/${target.id}/packages`}>Inventar öffnen <span className="ml-2" aria-hidden="true">→</span></Link></div><div className="flex-1">{packageInventoryContent(inventory, stale)}</div></article>;
}

function packageInventorySummary(inventory: InventoryQuery) {
  if (inventory.isLoading) return "Wird geladen…";
  if (inventory.error) return "Fehler beim Laden";
  if (inventory.data?.status !== "complete") return "Noch nicht erhoben";
  const collected = inventory.data.collected_at ? new Date(inventory.data.collected_at).toLocaleString() : "unbekannt";
  return `${inventory.data.packages.length} Pakete · erhoben ${collected}`;
}

function packageInventoryContent(inventory: InventoryQuery, stale: boolean) {
  if (inventory.error) return <p className="font-semibold text-[var(--error)]" role="alert">{inventory.error.message}</p>;
  if (inventory.isLoading) return <p className="text-[var(--muted)]">Paketinventar wird geladen…</p>;
  if (inventory.data?.status !== "complete") return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Für dieses Ziel gibt es noch kein Paketinventar.</p>;
  if (stale) return <output className="block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Paketinventar ist älter als 24 Stunden.</output>;
  return <p className="text-[var(--muted)]">Paketinventar aktuell.</p>;
}

function TargetTelemetry({ telemetry, samples, latest }: Readonly<{ telemetry: TelemetryQuery; samples: TelemetrySample[]; latest: TelemetrySample | undefined }>) {
  return <article className="flex min-h-44 flex-col border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><div className="mb-3 flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 className="mb-1">Systemauslastung</h2><p className="mb-0 text-sm text-[var(--muted)]">{telemetry.isLoading ? "Wird geladen…" : `${samples.length} Messpunkte im 30-Sekunden-Fenster`}</p></div><span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2 py-1 text-xs text-[var(--muted)]" title="Die Telemetrieansicht aktualisiert sich automatisch alle fünf Sekunden.">Live · 5 s</span></div><div className="flex-1">{targetTelemetryContent(telemetry, samples, latest)}</div></article>;
}

function targetTelemetryContent(telemetry: TelemetryQuery, samples: TelemetrySample[], latest: TelemetrySample | undefined) {
  if (telemetry.error) return <p className="font-semibold text-[var(--error)]" role="alert">{telemetry.error.message}</p>;
  if (telemetry.isLoading && samples.length === 0) return <p className="text-[var(--muted)]">Telemetrie wird geladen…</p>;
  if (samples.length === 0 || latest === undefined) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Telemetrie verfügbar.</p>;
  return <><LatestTelemetryNotice latest={latest} /><p className="mb-3 text-sm text-[var(--muted)]">Letzter Messpunkt: {new Date(latest.collected_at).toLocaleString()} · CPU {formatPercent(latest.cpu_basis_points)} · RAM {formatPercent(latest.memory_basis_points)} · Speicher {formatPercent(latest.storage_basis_points)}</p><div className="grid gap-4 md:grid-cols-3"><TelemetryChart label="CPU" field="cpu_basis_points" samples={samples} /><TelemetryChart label="RAM" field="memory_basis_points" samples={samples} /><TelemetryChart label="Speicher" field="storage_basis_points" samples={samples} /></div></>;
}

function LatestTelemetryNotice({ latest }: Readonly<{ latest: TelemetrySample }>) {
  if (Date.now() - new Date(latest.collected_at).getTime() <= staleAfterMs) return null;
  return <output className="mb-3 block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Messwerte sind älter als 2 Minuten; letzter Messpunkt {new Date(latest.collected_at).toLocaleString()}.</output>;
}

function TargetDockerInventory({ target, host, workloads, discovery, stale }: Readonly<{ target: Target; host: ContainerItem | undefined; workloads: DockerWorkloadsQuery; discovery: DockerDiscoveryQuery; stale: boolean }>) {
  if (target.kind !== "lxc") return null;
  return <article className="flex min-h-36 flex-col border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)] xl:col-span-2"><div className="mb-3 flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 className="mb-1">Docker-Inventar</h2><p className="mb-0 text-sm text-[var(--muted)]">Docker-Container werden getrennt vom LXC-Inventar geführt.</p></div>{host ? <Link className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={`/docker?host=${host.id}`}>Docker-Inventar öffnen <span className="ml-2" aria-hidden="true">→</span></Link> : null}</div><div className="flex-1">{targetDockerContent(host, workloads, discovery, stale)}</div></article>;
}

function targetDockerContent(host: ContainerItem | undefined, workloads: DockerWorkloadsQuery, discovery: DockerDiscoveryQuery, stale: boolean) {
  if (host === undefined) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Kein zugehöriger LXC-Host im Container-Inventar gefunden.</p>;
  if (workloads.isLoading || discovery.isLoading) return <p className="text-[var(--muted)]">Docker-Inventar wird geladen…</p>;
  if (workloads.error || discovery.error) return <p className="font-semibold text-[var(--error)]" role="alert">{workloads.error?.message ?? discovery.error?.message}</p>;
  return <><DockerStaleNotice stale={stale} />{dockerWorkloadList(workloads.data ?? [])}</>;
}

function DockerStaleNotice({ stale }: Readonly<{ stale: boolean }>) {
  if (!stale) return null;
  return <output className="mb-3 block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Letzte Docker-Erkennung ist älter als 15 Minuten.</output>;
}

function dockerWorkloadList(workloads: NonNullable<DockerWorkloadsQuery["data"]>) {
  if (workloads.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch kein Docker-Inventar für diesen Host.</p>;
  return <ul className="m-0 min-h-0 flex-1 list-none overflow-y-auto p-0">{workloads.slice(0, 5).map((workload) => <li className="flex items-center justify-between gap-3 border-b border-[var(--line)] py-2 last:border-0" key={workload.id}><span><strong>{workload.name}</strong><small className={`block ${"text-[var(--muted)]"}`}>{workload.image}</small></span><span className="text-[var(--muted)]">{workload.presence === "missing" ? "Nicht mehr vorhanden" : workload.status}</span></li>)}</ul>;
}

function TargetWorkflowList({ targetId, jobs, targetJobs }: Readonly<{ targetId: string; jobs: ReturnType<typeof useAnsibleJobs>; targetJobs: NonNullable<ReturnType<typeof useAnsibleJobs>["data"]> }>) {
  return <section className="border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><div className="mb-3 flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 className="mb-1">Letzte Workflows</h2><p className="mb-0 text-sm text-[var(--muted)]">Ausführungen für dieses Ziel</p></div><div className="flex items-center gap-3"><span className="text-xs text-[var(--muted)]">{jobs.isLoading ? "…" : `${targetJobs.length} angezeigt`}</span><Link className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={`/workflows?target=${encodeURIComponent(targetId)}`}>Alle Workflows <span className="ml-2" aria-hidden="true">→</span></Link></div></div>{targetWorkflowContent(jobs, targetJobs)}</section>;
}

function targetWorkflowContent(jobs: ReturnType<typeof useAnsibleJobs>, targetJobs: NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>) {
  if (jobs.isLoading) return <p className="text-[var(--muted)]">Workflows werden geladen…</p>;
  if (jobs.error) return <p className="font-semibold text-[var(--error)]" role="alert">{jobs.error.message}</p>;
  if (targetJobs.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Keine Workflows vorhanden.</p>;
  return <ul className="m-0 grid list-none gap-2 p-0">{targetJobs.map((job) => <li className="group flex flex-wrap items-center justify-between gap-3 border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-2.5 transition-colors hover:border-[var(--primary)]/50 hover:bg-[var(--primary-soft)]/30" key={job.id}><Link className="flex min-w-0 flex-1 items-center gap-3 text-sm font-semibold text-lxcup-primary hover:underline" to={`/workflows/${job.id}`}><span className={cn("grid h-9 w-9 shrink-0 place-items-center rounded-md border transition-transform group-hover:scale-105", activityStatusTone(job.status))} aria-hidden="true"><ActivityIcon name={activityIconForOperation(job.operation)} className="h-5 w-5" /></span><span className="min-w-0"><span className="block">{targetOperationLabel(job.operation)}</span><small className="mt-0.5 block font-normal text-[var(--muted)]" title={`Job ${job.id}`}>{job.updated_at ? `Ausgeführt · ${new Date(job.updated_at).toLocaleString()}` : `Job ${job.id.slice(0, 8)}`}</small></span></Link><span className={cn("inline-flex shrink-0 items-center px-2.5 py-1 text-xs font-bold", jobStatusBadgeClass(job.status))}>{jobStatusLabel(job.status)}</span></li>)}</ul>;
}

function targetOperationLabel(operation: string) {
  const labels: Record<string, string> = {
    deploy_agent: "Agent installieren",
    update_agent: "Agent aktualisieren",
    repair_agent: "Agent reparieren",
    update_packages: "Pakete aktualisieren",
    health_check: "Healthcheck",
    collect_package_inventory: "Paketinventar erfassen",
    configure_target: "Ziel konfigurieren",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}

type TelemetryField = "cpu_basis_points" | "memory_basis_points" | "storage_basis_points";
type TelemetrySample = NonNullable<ReturnType<typeof useTargetTelemetry>["data"]>["samples"][number];

function formatPercent(value: number | null | undefined) {
  return value == null ? "—" : `${(value / 100).toFixed(1)}%`;
}

function TelemetryChart({ label, field, samples }: Readonly<{ label: string; field: TelemetryField; samples: TelemetrySample[] }>) {
  const points = samples.map((sample, index) => ({
    x: samples.length <= 1 ? 10 : 10 + (index / (samples.length - 1)) * 580,
    y: telemetryY(sample[field]),
    sample,
  }));
  const description = `${label}-Auslastung im Verlauf der letzten 30 Sekunden; ${samples.length} Messpunkte.`;
  return <figure aria-label={description} className="min-w-0 rounded border border-[var(--line)] p-3">
    <figcaption className="mb-2 flex justify-between gap-2"><strong>{label}</strong><span className="sr-only">{description}</span><span className="text-sm text-[var(--muted)]">{formatPercent(samples.at(-1)?.[field])}</span></figcaption>
    <svg className="h-28 w-full" viewBox="0 0 600 160" aria-hidden="true">
      <title>{description}</title>
      <line x1="10" y1="20" x2="590" y2="20" stroke="currentColor" opacity="0.15" />
      <line x1="10" y1="80" x2="590" y2="80" stroke="currentColor" opacity="0.15" />
      <line x1="10" y1="140" x2="590" y2="140" stroke="currentColor" opacity="0.15" />
      <text x="10" y="14" fill="currentColor" fontSize="11">100%</text>
      <text x="10" y="155" fill="currentColor" fontSize="11">0%</text>
      {points.map((point, index) => {
        const previous = points[index - 1];
        return previous?.y != null && point.y != null ? <line key={`line-${point.sample.collected_at}`} x1={previous.x} y1={previous.y} x2={point.x} y2={point.y} stroke="var(--accent)" strokeWidth="3" /> : null;
      })}
      {points.map((point) => point.y == null ? null : <circle key={point.sample.collected_at} cx={point.x} cy={point.y} r="3.5" fill="var(--accent)"><title>{new Date(point.sample.collected_at).toLocaleTimeString()}: {formatPercent(point.sample[field])}</title></circle>)}
    </svg>
    <div className="flex justify-between text-xs text-[var(--muted)]" aria-hidden="true"><span>{new Date(samples[0].collected_at).toLocaleTimeString()}</span><span>{new Date(samples.at(-1)!.collected_at).toLocaleTimeString()}</span></div>
    <ul className="sr-only">{samples.map((sample) => <li key={sample.collected_at}>{new Date(sample.collected_at).toLocaleString()}: {formatPercent(sample[field])}</li>)}</ul>
  </figure>;
}

function telemetryY(value: number | null | undefined) {
  if (value == null) return null;
  return 140 - (Math.max(0, Math.min(10_000, value)) / 10_000) * 120;
}
