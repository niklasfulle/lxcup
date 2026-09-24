import { Link, useParams } from "react-router-dom";
import { cn, ui } from "../ui";
import { useAnsibleJobs, useContainers, useDockerDiscovery, useDockerWorkloads, usePackageInventory, useTargetTelemetry, useTargets } from "../queries";

function stateLabel(state: string) {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Ausstehend";
}

export function TargetDetailPage() {
  const { targetId } = useParams();
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
  const samples = telemetry.data?.samples ?? [];
  const latest = samples.at(-1);
  const heartbeatAge = target ? Date.now() - new Date(target.updated_at).getTime() : 0;
  const stale = heartbeatAge > 15_000 || Boolean(latest && Date.now() - new Date(latest.collected_at).getTime() > 15_000);
  const inventoryStale = Boolean(inventory.data?.collected_at && Date.now() - new Date(inventory.data.collected_at).getTime() > 24 * 60 * 60 * 1000);
  const dockerStale = Boolean(dockerDiscovery.data?.finished_at && Date.now() - new Date(dockerDiscovery.data.finished_at).getTime() > 15 * 60 * 1000);

  if (targets.isLoading) return <section className={ui.panel}><p className={ui.muted}>Ziel wird geladen…</p></section>;
  if (!target) return <section className={ui.panel}><p className={ui.errorState}>Ziel nicht gefunden.</p><Link className={ui.textLink} to="/targets">Zur Zielübersicht</Link></section>;

  return <>
    <header className={ui.pageHeader}>
      <div><p className={ui.eyebrow}>{target.kind === "lxc" ? "LXC-Container" : target.kind === "windows_server" ? "Windows" : "Server"}</p><h1>{target.name}</h1><p className={ui.muted}>{target.address} · {target.transport.toUpperCase()}</p></div>
      <Link className={ui.button} to={target.kind === "lxc" ? "/containers" : target.kind === "windows_server" ? "/windows" : "/servers"}>← Ressourcenübersicht</Link>
    </header>
    <section className={ui.summaryGrid}>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Lifecycle</span><strong className={cn(ui.statusBadge, target.state === "managed" ? ui.statusSuccess : ui.statusPending)}>{stateLabel(target.state)}</strong></article>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Agent-Version</span><strong>{target.agent_version ?? "Noch nicht gemeldet"}</strong></article>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Letzter Heartbeat</span><strong>{new Date(target.updated_at).toLocaleString()}</strong><small className={ui.muted}>{stale ? "Veraltet" : "Aktuell"}</small></article>
    </section>
    {stale ? <div className={cn(ui.callout, ui.calloutDanger)} role="status">Der letzte Heartbeat ist älter als 15 Sekunden. Telemetrie und Agentstatus können veraltet sein.</div> : null}
    <section className={ui.panelGrid}>
      <article className={ui.panel}><div className={ui.sectionHeading}><div><h2>Paketinventar</h2><p className={ui.muted}>{inventory.isLoading ? "Wird geladen…" : inventory.error ? "Fehler beim Laden" : inventory.data?.status === "complete" ? `${inventory.data.packages.length} Pakete · erhoben ${inventory.data.collected_at ? new Date(inventory.data.collected_at).toLocaleString() : "unbekannt"}` : "Noch nicht erhoben"}</p></div><Link className={ui.button} to={`/targets/${target.id}/packages`}>Inventar öffnen</Link></div>{inventory.error ? <p className={ui.errorState} role="alert">{inventory.error.message}</p> : inventory.isLoading ? <p className={ui.muted}>Paketinventar wird geladen…</p> : inventory.data?.status !== "complete" ? <p className={ui.emptyState}>Für dieses Ziel gibt es noch kein Paketinventar.</p> : inventoryStale ? <p className={ui.callout} role="status">Paketinventar ist älter als 24 Stunden.</p> : <p className={ui.muted}>Paketinventar aktuell.</p>}</article>
      <article className={ui.panel}>
        <div className={ui.sectionHeading}>
          <div><h2>Systemauslastung</h2><p className={ui.muted}>{telemetry.isLoading ? "Wird geladen…" : `${samples.length} Samples im 30-Sekunden-Fenster`}</p></div>
          <span className={ui.muted} title="Die Telemetrieansicht aktualisiert sich automatisch alle fünf Sekunden.">Live · 5 s</span>
        </div>
        {telemetry.error ? <p className={ui.errorState} role="alert">{telemetry.error.message}</p> : telemetry.isLoading && samples.length === 0 ? <p className={ui.muted}>Telemetrie wird geladen…</p> : samples.length === 0 ? <p className={ui.emptyState}>Noch keine Telemetrie verfügbar.</p> : <>
          {latest && Date.now() - new Date(latest.collected_at).getTime() > 15_000 ? <p className={ui.callout} role="status">Messwerte sind veraltet; letzter Messpunkt {new Date(latest.collected_at).toLocaleString()}.</p> : null}
          <p className="mb-3 text-sm text-[var(--muted)]">Letzter Messpunkt: {new Date(latest!.collected_at).toLocaleString()} · CPU {formatPercent(latest!.cpu_basis_points)} · RAM {formatPercent(latest!.memory_basis_points)} · Speicher {formatPercent(latest!.storage_basis_points)}</p>
          <div className="grid gap-4 md:grid-cols-3">
            <TelemetryChart label="CPU" field="cpu_basis_points" samples={samples} />
            <TelemetryChart label="RAM" field="memory_basis_points" samples={samples} />
            <TelemetryChart label="Speicher" field="storage_basis_points" samples={samples} />
          </div>
        </>}
      </article>
      {target.kind === "lxc" ? <article className={ui.panel}><div className={ui.sectionHeading}><div><h2>Docker-Inventar</h2><p className={ui.muted}>Docker-Container werden getrennt vom LXC-Inventar geführt.</p></div>{hostContainer ? <Link className={ui.button} to={`/docker?host=${hostContainer.id}`}>Docker-Inventar dieses Hosts</Link> : null}</div>{!hostContainer ? <p className={ui.emptyState}>Kein zugehöriger LXC-Host im Container-Inventar gefunden.</p> : dockerWorkloads.isLoading || dockerDiscovery.isLoading ? <p className={ui.muted}>Docker-Inventar wird geladen…</p> : dockerWorkloads.error || dockerDiscovery.error ? <p className={ui.errorState} role="alert">{dockerWorkloads.error?.message ?? dockerDiscovery.error?.message}</p> : <>{dockerStale ? <p className={ui.callout} role="status">Letzte Docker-Erkennung ist älter als 15 Minuten.</p> : null}{dockerWorkloads.data?.length ? <ul className={ui.activityList}>{dockerWorkloads.data.slice(0, 5).map((workload) => <li className="flex items-center justify-between gap-3 border-b border-[var(--line)] py-2 last:border-0" key={workload.id}><span><strong>{workload.name}</strong><small className={`block ${ui.muted}`}>{workload.image}</small></span><span className={ui.muted}>{workload.presence === "missing" ? "Nicht mehr vorhanden" : workload.status}</span></li>)}</ul> : <p className={ui.emptyState}>Noch kein Docker-Inventar für diesen Host.</p>}</>}</article> : null}
    </section>
    <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Letzte Workflows</h2><p className={ui.muted}>Nur Jobs dieses Ziels</p></div><Link className={ui.button} to={`/workflows?target=${encodeURIComponent(target.id)}`}>Workflows dieses Ziels</Link></div>{jobs.isLoading ? <p className={ui.muted}>Workflows werden geladen…</p> : jobs.error ? <p className={ui.errorState} role="alert">{jobs.error.message}</p> : targetJobs.length === 0 ? <p className={ui.emptyState}>Keine Workflows vorhanden.</p> : <ul className={ui.activityList}>{targetJobs.map((job) => <li className="flex items-center justify-between gap-3 border-b border-[var(--line)] py-2 last:border-0" key={job.id}><Link className={ui.textLink} to={`/workflows/${job.id}`}>{job.operation.replaceAll("_", " ")}</Link><span className={cn(ui.statusBadge, job.status === "succeeded" ? ui.statusSuccess : job.status === "failed" ? ui.calloutDanger : ui.statusPending)}>{job.status}</span></li>)}</ul>}</section>
  </>;
}

type TelemetryField = "cpu_basis_points" | "memory_basis_points" | "storage_basis_points";
type TelemetrySample = NonNullable<ReturnType<typeof useTargetTelemetry>["data"]>["samples"][number];

function formatPercent(value: number | null | undefined) {
  return value == null ? "—" : `${(value / 100).toFixed(1)}%`;
}

function TelemetryChart({ label, field, samples }: Readonly<{ label: string; field: TelemetryField; samples: TelemetrySample[] }>) {
  const points = samples.map((sample, index) => ({
    x: samples.length <= 1 ? 10 : 10 + (index / (samples.length - 1)) * 580,
    y: sample[field] == null ? null : 140 - (Math.max(0, Math.min(10_000, sample[field]!)) / 10_000) * 120,
    sample,
  }));
  const description = `${label}-Auslastung im Verlauf der letzten 30 Sekunden; ${samples.length} Messpunkte.`;
  return <figure className="min-w-0 rounded border border-[var(--line)] p-3">
    <figcaption className="mb-2 flex justify-between gap-2"><strong>{label}</strong><span className="text-sm text-[var(--muted)]">{formatPercent(samples.at(-1)?.[field])}</span></figcaption>
    <svg className="h-28 w-full" viewBox="0 0 600 160" role="img" aria-label={description}>
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
