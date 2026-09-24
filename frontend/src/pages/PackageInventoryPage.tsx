import { useDeferredValue, useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { cn, ui } from "../ui";
import { usePackageInventory, useTargetTelemetry, useTargets } from "../queries";

type TelemetrySample = NonNullable<ReturnType<typeof useTargetTelemetry>["data"]>["samples"][number];

function TelemetryChart({ samples }: { samples: TelemetrySample[] }) {
  const width = 720;
  const height = 180;
  const padding = 24;
  const x = (index: number) => padding + (index * (width - padding * 2)) / Math.max(samples.length - 1, 1);
  const y = (value: number) => height - padding - (value / 10000) * (height - padding * 2);
  const path = (read: (sample: TelemetrySample) => number | null) => samples
    .map((sample, index) => {
      const value = read(sample);
      return value == null ? null : `${index === 0 ? "M" : "L"} ${x(index)} ${y(value)}`;
    })
    .filter((point): point is string => point !== null)
    .join(" ");
  return <div className="overflow-x-auto border border-[var(--line)] bg-[var(--paper-muted)] p-2">
    <svg className="min-w-[34rem]" viewBox={`0 0 ${width} ${height}`} role="img" aria-label="CPU-, RAM- und Speicherauslastung der letzten 30 Sekunden">
      {[0, 25, 50, 75, 100].map((percent) => <g key={percent}><line x1={padding} x2={width - padding} y1={y(percent * 100)} y2={y(percent * 100)} stroke="currentColor" opacity=".16" /><text x="2" y={y(percent * 100) + 4} fontSize="10" fill="currentColor" opacity=".7">{percent}%</text></g>)}
      <path d={path((sample) => sample.cpu_basis_points)} fill="none" stroke="#60a5fa" strokeWidth="2" />
      <path d={path((sample) => sample.memory_basis_points)} fill="none" stroke="#34d399" strokeWidth="2" />
      <path d={path((sample) => sample.storage_basis_points)} fill="none" stroke="#fbbf24" strokeWidth="2" />
    </svg>
    <div className="flex flex-wrap gap-3 px-2 text-xs text-[var(--muted)]"><span className="text-blue-400">● CPU</span><span className="text-emerald-400">● RAM</span><span className="text-amber-400">● Speicher</span></div>
  </div>;
}

export function PackageInventoryPage() {
  const { targetId } = useParams();
  const inventory = usePackageInventory(targetId);
  const telemetry = useTargetTelemetry(targetId);
  const targets = useTargets();
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim().toLowerCase());
  const [sortBy, setSortBy] = useState<"name" | "version">("name");
  const target = targets.data?.find((item) => item.id === targetId);
  const latestSample = telemetry.data?.samples.at(-1);
  const telemetryStale = latestSample ? Date.now() - new Date(latestSample.collected_at).getTime() > 15_000 : false;
  const packages = useMemo(() => {
    const all = inventory.data?.packages ?? [];
    return [...all]
      .filter((item) => !deferredQuery || item.name.toLowerCase().includes(deferredQuery) || item.installed_version.toLowerCase().includes(deferredQuery))
      .sort((left, right) => sortBy === "name" ? left.name.localeCompare(right.name) : left.installed_version.localeCompare(right.installed_version));
  }, [deferredQuery, inventory.data?.packages, sortBy]);

  return <>
    <header className={ui.pageHeader}><div><p className={ui.eyebrow}>Systeminventar</p><h1>Paketinventar</h1><p className={ui.muted}>{target ? `${target.name} · ${target.kind}` : "Installierte Pakete des verwalteten Systems"}</p></div><Link className={ui.button} to={target?.kind === "lxc" ? "/containers" : target?.kind === "windows_server" ? "/windows" : "/servers"}>← Zur Ressourcenübersicht</Link></header>
    <section className={ui.panel}>
      <div className={ui.sectionHeading}><div><h2>Systemauslastung</h2><p className={ui.muted}>Letzte 30 Sekunden · {telemetry.data?.samples.length ?? 0} Samples</p></div></div>
      {telemetryStale ? <p className={cn(ui.callout, ui.calloutDanger)} role="status">Telemetrie ist nicht aktuell. Der letzte Messwert ist älter als 15 Sekunden; Heartbeat und Agent-Verbindung prüfen.</p> : null}
      {telemetry.isLoading ? <p className={ui.muted}>Telemetrie wird geladen…</p> : telemetry.error ? <p className={ui.errorState} role="alert">{telemetry.error.message}</p> : telemetry.data?.samples.length ? <><TelemetryChart samples={telemetry.data.samples} /><div className={ui.tableWrap}><table><thead><tr><th>Zeit</th><th>CPU</th><th>RAM</th><th>Speicher</th><th>Load</th></tr></thead><tbody>{telemetry.data.samples.map((sample) => <tr key={sample.collected_at}><td>{new Date(sample.collected_at).toLocaleTimeString()}</td><td>{sample.cpu_basis_points == null ? "—" : `${(sample.cpu_basis_points / 100).toFixed(1)}%`}</td><td>{sample.memory_basis_points == null ? "—" : `${(sample.memory_basis_points / 100).toFixed(1)}%`}</td><td>{sample.storage_basis_points == null ? "—" : `${(sample.storage_basis_points / 100).toFixed(1)}%`}</td><td>{sample.load_1_milli == null ? "—" : (sample.load_1_milli / 1000).toFixed(2)}</td></tr>)}</tbody></table></div></> : <p className={ui.emptyState}>Noch keine Telemetrie verfügbar.</p>}
    </section>
    <section className={ui.panel}>
      {inventory.isLoading ? <p className={ui.muted}>Paketinventar wird geladen…</p> : null}
      {inventory.error ? <p className={ui.errorState} role="alert">{inventory.error.message}</p> : null}
      {inventory.data?.status === "not_collected" ? <div className={cn(ui.callout, ui.calloutInfo)}><strong>Noch kein Paketinventar</strong><p>Die erste Inventarisierung wird nach dem Onboarding eingereiht. Anschließend erscheint hier die vollständige, durchsuchbare Paketliste.</p></div> : null}
      {inventory.data?.status === "complete" ? <>
        <div className={ui.sectionHeading}><div><h2>{inventory.data.packages.length} installierte Pakete</h2><p className={ui.muted}>Zuletzt erhoben: {inventory.data.collected_at ? new Date(inventory.data.collected_at).toLocaleString() : "unbekannt"}</p></div></div>
        <div className={ui.workflowGrid}><label><span>Suche nach Paket oder Version</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="z. B. curl oder 8.5" /></label><label><span>Sortierung</span><select value={sortBy} onChange={(event) => setSortBy(event.target.value as "name" | "version")}><option value="name">Paketname</option><option value="version">Installierte Version</option></select></label></div>
        {packages.length === 0 ? <p className={ui.emptyState}>Keine Pakete entsprechen dem aktuellen Filter.</p> : <div className={ui.tableWrap}><table><thead><tr><th>Paket</th><th>Installierte Version</th><th>Architektur</th><th>Quelle</th></tr></thead><tbody>{packages.map((item) => <tr key={`${item.name}-${item.installed_version}-${item.architecture ?? ""}`}><td><strong>{item.name}</strong></td><td>{item.installed_version}</td><td>{item.architecture ?? "—"}</td><td>{item.source ?? "—"}</td></tr>)}</tbody></table></div>}
      </> : null}
    </section>
  </>;
}
