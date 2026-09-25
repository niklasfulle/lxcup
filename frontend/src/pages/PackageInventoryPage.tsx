import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { cn } from "../classnames";
import { createAnsibleJob, type AnsibleJobDto } from "../api";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { queryKeys, useAnsibleJobs, usePackageInventory, useTargetTelemetry, useTargets } from "../queries";
import { isTelemetryStale, telemetryAgeLabel } from "../telemetryFreshness";

type TelemetrySample = NonNullable<ReturnType<typeof useTargetTelemetry>["data"]>["samples"][number];

type TelemetryQuery = ReturnType<typeof useTargetTelemetry>;

function pathCommand(index: number) {
  return index === 0 ? "M" : "L";
}

function targetListPath(kind: string | undefined) {
  if (kind === "lxc") return "/containers";
  if (kind === "windows_server") return "/windows";
  return "/servers";
}

function inventoryStatusLabel(status: string | undefined, loading: boolean) {
  if (status === "complete") return "Abgeschlossen";
  if (status === "not_collected") return "Noch nicht erhoben";
  return loading ? "Wird geladen…" : "Unbekannt";
}

function TelemetryChart({ samples }: Readonly<{ samples: TelemetrySample[] }>) {
  const width = 720;
  const height = 180;
  const padding = 24;
  const x = (index: number) => padding + (index * (width - padding * 2)) / Math.max(samples.length - 1, 1);
  const y = (value: number) => height - padding - (value / 10000) * (height - padding * 2);
  const path = (read: (sample: TelemetrySample) => number | null) => samples
    .map((sample, index) => {
      const value = read(sample);
      return value == null ? null : `${pathCommand(index)} ${x(index)} ${y(value)}`;
    })
    .filter((point): point is string => point !== null)
    .join(" ");
  return <figure aria-label="CPU-, RAM- und Speicherauslastung der letzten 30 Sekunden" className="overflow-x-auto border border-[var(--line)] bg-[var(--paper-muted)] p-2">
    <figcaption className="sr-only">CPU-, RAM- und Speicherauslastung der letzten 30 Sekunden, jeweils als separate Linie.</figcaption>
    <svg className="min-w-[34rem]" viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      {[0, 25, 50, 75, 100].map((percent) => <g key={percent}><line x1={padding} x2={width - padding} y1={y(percent * 100)} y2={y(percent * 100)} stroke="currentColor" opacity=".16" /><text x="2" y={y(percent * 100) + 4} fontSize="10" fill="currentColor" opacity=".7">{percent}%</text></g>)}
      <path d={path((sample) => sample.cpu_basis_points)} fill="none" stroke="#60a5fa" strokeWidth="2" />
      <path d={path((sample) => sample.memory_basis_points)} fill="none" stroke="#34d399" strokeWidth="2" />
      <path d={path((sample) => sample.storage_basis_points)} fill="none" stroke="#fbbf24" strokeWidth="2" />
    </svg>
    <div className="flex flex-wrap gap-3 px-2 text-xs text-[var(--muted)]"><span className="text-blue-400">● CPU</span><span className="text-emerald-400">● RAM</span><span className="text-amber-400">● Speicher</span></div>
  </figure>;
}

export function PackageInventoryPage() {
  const { targetId } = useParams();
  const [now, setNow] = useState(() => Date.now());
  const queryClient = useQueryClient();
  const inventory = usePackageInventory(targetId);
  const telemetry = useTargetTelemetry(targetId);
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const [submittedInventoryJob, setSubmittedInventoryJob] = useState<AnsibleJobDto>();
  const refreshedInventoryJob = useRef<string | undefined>(undefined);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim().toLowerCase());
  const [sortBy, setSortBy] = useState<"name" | "version">("name");
  const target = targets.data?.find((item) => item.id === targetId);
  const inventoryJobs = (jobs.data ?? [])
    .filter((job) => job.operation === "collect_package_inventory" && "target" in job.target && job.target.target === targetId)
    .sort((left, right) => right.created_at.localeCompare(left.created_at));
  const inventoryJob = submittedInventoryJob
    ? inventoryJobs.find((job) => job.id === submittedInventoryJob.id) ?? submittedInventoryJob
    : inventoryJobs[0];
  const hasActiveInventoryJob = inventoryJobs.some((job) => isActiveInventoryJob(job.status)) || Boolean(submittedInventoryJob && !inventoryJobs.some((job) => job.id === submittedInventoryJob.id) && isActiveInventoryJob(submittedInventoryJob.status));
  const latestSample = telemetry.data?.samples.at(-1);
  const telemetryStale = latestSample ? isTelemetryStale(latestSample.collected_at, now) : false;
  const collectInventory = useMutation({
    mutationFn: () => createAnsibleJob({
      operation: "collect_package_inventory",
      target_id: targetId,
      mode: "check",
      parameters: { operation: "collect_package_inventory" },
      idempotency_key: `inventory-refresh-${targetId}-${crypto.randomUUID()}`,
      confirmed: true,
    }),
    onSuccess: (job) => {
      setSubmittedInventoryJob(job);
      queryClient.setQueryData<AnsibleJobDto[]>(queryKeys.ansibleJobs, (current) => [job, ...(current ?? []).filter((item) => item.id !== job.id)]);
      void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs });
    },
  });

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 5_000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!targetId || !inventoryJob || inventoryJob.status !== "succeeded" || refreshedInventoryJob.current === inventoryJob.id) return;
    refreshedInventoryJob.current = inventoryJob.id;
    void queryClient.invalidateQueries({ queryKey: queryKeys.packageInventory(targetId) });
  }, [inventoryJob, queryClient, targetId]);
  const packages = useMemo(() => {
    const all = inventory.data?.packages ?? [];
    return [...all]
      .filter((item) => !deferredQuery || item.name.toLowerCase().includes(deferredQuery) || item.installed_version.toLowerCase().includes(deferredQuery))
      .sort((left, right) => sortBy === "name" ? left.name.localeCompare(right.name) : left.installed_version.localeCompare(right.installed_version));
  }, [deferredQuery, inventory.data?.packages, sortBy]);

  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Systeminventar</p><h1>Paketinventar</h1><p className="text-[var(--muted)]">{target ? `${target.name} · ${target.kind}` : "Installierte Pakete des verwalteten Systems"}</p></div><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to={targetListPath(target?.kind)}>← Zur Ressourcenübersicht</Link></header>
    <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">
      <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Systemauslastung</h2><p className="text-[var(--muted)]">Letzte 30 Sekunden · {telemetry.data?.samples.length ?? 0} Samples</p></div></div>
      {latestSample ? <p className="mb-2 text-xs text-[var(--muted)]" aria-live="polite">Letzter Messpunkt {telemetryAgeLabel(latestSample.collected_at, now)} · {telemetryStale ? "Veraltet" : "Aktuell"}</p> : null}
      {telemetryStale ? <output className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-red-200 bg-[var(--error-soft)]")}>Telemetrie ist veraltet. Seit dem letzten Messpunkt sind mehr als 2 Minuten vergangen; Heartbeat und Agent-Verbindung prüfen.</output> : null}
      {telemetryContent(telemetry)}
    </section>
    <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="grid gap-1"><span className="text-[var(--muted)]">Erhebungsstatus: {inventoryStatusLabel(inventory.data?.status, inventory.isLoading)}</span>
          {inventoryJob ? <Link className={cn("inline-flex w-fit items-center border border-transparent px-2 py-1 text-xs font-semibold hover:border-[var(--primary)] hover:underline", jobStatusBadgeClass(inventoryJob.status))} to={`/workflows/${inventoryJob.id}`}>Inventarisierungs-Workflow · {jobStatusLabel(inventoryJob.status)}</Link> : <span className="text-xs text-[var(--muted)]">Kein Inventarisierungs-Workflow vorhanden</span>}
        </div>
        {target ? <button className="inline-flex min-h-9 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={collectInventory.isPending || hasActiveInventoryJob || target.state !== "managed"} onClick={() => collectInventory.mutate()}>{collectInventory.isPending ? "Wird eingereiht…" : hasActiveInventoryJob ? "Inventarisierung läuft" : "Inventarisierung starten"}</button> : null}
      </div>
      {target && target.state !== "managed" ? <p className="text-xs text-[var(--muted)]">Die Erfassung ist erst möglich, wenn das Ziel verbunden ist.</p> : null}
      {collectInventory.error instanceof Error ? <p className="font-semibold text-[var(--error)]" role="alert">Inventarisierung konnte nicht gestartet werden: {collectInventory.error.message}</p> : null}
      {inventory.isLoading ? <p className="text-[var(--muted)]">Paketinventar wird geladen…</p> : null}
      {inventory.error ? <p className="font-semibold text-[var(--error)]" role="alert">{inventory.error.message}</p> : null}
      {inventory.data?.status === "not_collected" ? <div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}><strong>Noch kein Paketinventar</strong><p>Die erste Inventarisierung wird nach dem Onboarding eingereiht. Anschließend erscheint hier die vollständige, durchsuchbare Paketliste.</p></div> : null}
      {inventory.data?.status === "complete" ? <>
        <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>{inventory.data.packages.length} installierte Pakete</h2><p className="text-[var(--muted)]">Zuletzt erhoben: {inventory.data.collected_at ? new Date(inventory.data.collected_at).toLocaleString() : "unbekannt"}</p></div></div>
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3"><label><span>Suche nach Paket oder Version</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="z. B. curl oder 8.5" /></label><label><span>Sortierung</span><select value={sortBy} onChange={(event) => setSortBy(event.target.value as "name" | "version")}><option value="name">Paketname</option><option value="version">Installierte Version</option></select></label></div>
        {packages.length === 0 ? <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Keine Pakete entsprechen dem aktuellen Filter.</p> : <div className="overflow-x-auto"><table><thead><tr><th>Paket</th><th>Installierte Version</th><th>Architektur</th><th>Quelle</th></tr></thead><tbody>{packages.map((item) => <tr key={`${item.name}-${item.installed_version}-${item.architecture ?? ""}`}><td><strong>{item.name}</strong></td><td>{item.installed_version}</td><td>{item.architecture ?? "—"}</td><td>{item.source ?? "—"}</td></tr>)}</tbody></table></div>}
      </> : null}
    </section>
  </>;
}

function isActiveInventoryJob(status: string) {
  return ["queued", "checking", "planned", "applying"].includes(status);
}

function telemetryContent(telemetry: TelemetryQuery) {
  if (telemetry.isLoading) return <p className="text-[var(--muted)]">Telemetrie wird geladen…</p>;
  if (telemetry.error) return <p className="font-semibold text-[var(--error)]" role="alert">{telemetry.error.message}</p>;
  const samples = telemetry.data?.samples ?? [];
  if (samples.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Telemetrie verfügbar.</p>;
  return <><TelemetryChart samples={samples} /><div className="overflow-x-auto"><table><thead><tr><th>Zeit</th><th>CPU</th><th>RAM</th><th>Speicher</th><th>Load</th></tr></thead><tbody>{samples.map((sample) => <tr key={sample.collected_at}><td>{new Date(sample.collected_at).toLocaleTimeString()}</td><td>{formatBasisPoints(sample.cpu_basis_points)}</td><td>{formatBasisPoints(sample.memory_basis_points)}</td><td>{formatBasisPoints(sample.storage_basis_points)}</td><td>{sample.load_1_milli == null ? "—" : (sample.load_1_milli / 1000).toFixed(2)}</td></tr>)}</tbody></table></div></>;
}

function formatBasisPoints(value: number | null) {
  if (value == null) return "—";
  return `${(value / 100).toFixed(1)}%`;
}
