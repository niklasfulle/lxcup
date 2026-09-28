import { cn } from "../classnames";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { adoptDockerWorkload, applyTargetDockerImageUpdate, checkTargetDockerImageUpdate, discoverDockerWorkloads, discoverTargetDocker, operateTargetDockerContainer, removeDockerWorkload, type ContainerDto, type DockerImageUpdateResult, type DockerLifecycleAction, type DockerTelemetryDto, type TargetDockerDiscoveryDto, type TargetDto } from "../api";
import { queryKeys, useContainers, useDockerDiscovery, useDockerWorkloads, useTargetDockerInventories, useTargetDockerInventory, useTargetDockerTelemetry, useTargets } from "../queries";
import { dockerDiscoveryFailureLabel } from "../dockerDiscoveryStatus";

export function DockerPage() {
  const containers = useContainers();
  const targets = useTargets();
  const [searchParams, setSearchParams] = useSearchParams();
  const requestedHost = Number(searchParams.get("host"));
  const [hostId, setHostId] = useState<number | undefined>(() => Number.isSafeInteger(requestedHost) && requestedHost > 0 ? requestedHost : undefined);
  const [targetId, setTargetId] = useState<string | undefined>(() => searchParams.get("target") ?? undefined);
  const [targetDiscoveryResult, setTargetDiscoveryResult] = useState<TargetDockerDiscoveryDto>();
  const [hostSelectorOpen, setHostSelectorOpen] = useState(false);
  const [selectedDockerIds, setSelectedDockerIds] = useState<string[]>([]);
  const [bulkResults, setBulkResults] = useState<Array<{ id: string; success: boolean }>>([]);
  const [imageChecks, setImageChecks] = useState<Record<string, DockerImageUpdateResult>>({});
  const workloads = useDockerWorkloads(hostId);
  const discovery = useDockerDiscovery(hostId);
  const targetInventory = useTargetDockerInventory(targetId);
  const targetTelemetry = useTargetDockerTelemetry(targetId);
  const lxcTargets = (targets.data ?? []).filter((target) => target.kind === "lxc");
  const targetInventories = useTargetDockerInventories(lxcTargets.map((target) => target.id));
  const queryClient = useQueryClient();
  const refresh = () => {
    if (hostId) {
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerWorkloads(hostId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerDiscovery(hostId) });
    }
  };
  const discover = useMutation({ mutationFn: () => discoverDockerWorkloads(hostId!), onSuccess: refresh });
  const targetDiscover = useMutation({
    mutationFn: () => discoverTargetDocker(targetId!),
    onSuccess: async (result) => {
      setTargetDiscoveryResult(result);
      if (result.available) {
        const inventoryKey = queryKeys.targetDockerInventory(result.target_id);
        await queryClient.cancelQueries({ queryKey: inventoryKey });
        queryClient.setQueryData(inventoryKey, result);
      }
    },
  });
  const lifecycle = useMutation({
    mutationFn: ({ containerId, action }: { containerId: string; action: DockerLifecycleAction }) => operateTargetDockerContainer(targetId!, containerId, action),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.targetDockerInventory(targetId!) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.targetDockerTelemetry(targetId!) }),
      ]);
    },
  });
  const imageUpdateCheck = useMutation({
    mutationFn: (containerId: string) => checkTargetDockerImageUpdate(targetId!, containerId),
    onSuccess: ({ result }) => setImageChecks((current) => ({ ...current, [result.container_id]: result })),
  });
  const imageUpdateApply = useMutation({
    mutationFn: ({ containerId, expectedRemoteImageId }: { containerId: string; expectedRemoteImageId: string }) => applyTargetDockerImageUpdate(targetId!, containerId, expectedRemoteImageId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.targetDockerInventory(targetId!) });
    },
  });
  const adopt = useMutation({ mutationFn: (dockerId: string) => adoptDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const remove = useMutation({ mutationFn: (dockerId: string) => removeDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const [removeCandidate, setRemoveCandidate] = useState<{ id: string; name: string } | null>(null);
  const host = (containers.data ?? []).find((container) => container.id === hostId);
  const targetHost = (targets.data ?? []).find((target) => target.id === targetId);
  const availableLxcTargets = lxcTargets.filter((target) => target.state === "managed");
  const targetSelected = Boolean(targetId);
  const containerSelected = Boolean(hostId);
  const inventoryCount = targetSelected
    ? (targetDiscoveryResult ?? targetInventory.data)?.containers.length ?? 0
    : containerSelected
      ? workloads.data?.length ?? 0
      : targetInventories.reduce((count, inventory) => count + (inventory.data?.available ? inventory.data.containers.length : 0), 0);

  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Agent-Inventar</p><h1>Docker-Container</h1><p className="text-[var(--muted)]">Erkannte Docker-Container auf eingebundenen LXC-Agenten aufnehmen und verwalten.</p></div><button className={hostSelectorOpen ? "inline-flex min-h-9 items-center justify-center gap-2 border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" : "inline-flex min-h-9 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white transition-colors hover:bg-blue-700"} type="button" aria-expanded={hostSelectorOpen} aria-controls="docker-host-selector" onClick={() => setHostSelectorOpen((open) => !open)}>{hostSelectorOpen ? "Schließen" : "Hinzufügen"}</button></header>
    {hostSelectorOpen ? <DockerHostSelector targets={availableLxcTargets} containers={containers.data ?? []} value={targetId ? `target:${targetId}` : hostId ?? ""} onSelectTarget={(nextTargetId) => { setTargetId(nextTargetId); setHostId(undefined); setTargetDiscoveryResult(undefined); setSearchParams({ target: nextTargetId }); setHostSelectorOpen(false); }} onSelectContainer={(nextHostId) => { setTargetId(undefined); setTargetDiscoveryResult(undefined); setHostId(nextHostId); setSearchParams(nextHostId ? { host: String(nextHostId) } : {}); if (nextHostId !== undefined) setHostSelectorOpen(false); }} /> : null}
    {targetSelected || containerSelected ? <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
      <SelectedDockerHost targetHost={targetHost} host={host} targetDiscoveryPending={targetDiscover.isPending} discoveryPending={discover.isPending} onDiscoverTarget={() => targetDiscover.mutate()} onDiscoverContainer={() => discover.mutate()} />
      {targetSelected ? <TargetDockerResult discovery={targetDiscoveryResult ?? targetInventory.data} error={targetDiscover.error ?? targetInventory.error} /> : null}
      {containerSelected && discover.error ? <p className="font-semibold text-[var(--error)]" role="alert">{discover.error.message}</p> : null}
      {containerSelected ? <DockerDiscoverySummary hostId={hostId} hostName={host?.name} discovery={discovery} /> : null}
    </section> : null}
    <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">
      <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Docker-Inventar</h2><p className="text-[var(--muted)]">„Aufnehmen“ verwaltet den Inventareintrag in lxcup; der Docker-Container bleibt unverändert.</p></div><span className="text-[var(--muted)]">{inventoryCount} Container</span></div>
      {targetSelected && lifecycle.error ? <p className="font-semibold text-[var(--error)]" role="alert">Docker-Aktion fehlgeschlagen: {lifecycle.error.message}</p> : null}
      {targetSelected ? <>{bulkResults.length > 0 ? <div className="mb-3 border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-sm" role="status"><strong>Bulk-Ergebnis:</strong> {bulkResults.filter((result) => result.success).length} erfolgreich · {bulkResults.filter((result) => !result.success).length} fehlgeschlagen</div> : null}<DockerBulkActionBar selectedCount={selectedDockerIds.length} pending={lifecycle.isPending} onClear={() => setSelectedDockerIds([])} onSelectAll={() => setSelectedDockerIds((targetDiscoveryResult ?? targetInventory.data)?.containers.map((item) => item.id) ?? [])} onAction={async (action) => {
        const actionLabels: Record<DockerLifecycleAction, string> = { start: "starten", stop: "stoppen", restart: "neu starten" };
        if (!window.confirm(`${selectedDockerIds.length} Docker-Container wirklich ${actionLabels[action]}?`)) return;
        setBulkResults([]);
        const results: Array<{ id: string; success: boolean }> = [];
        for (const containerId of selectedDockerIds) {
          try { await lifecycle.mutateAsync({ containerId, action }); results.push({ id: containerId, success: true }); }
          catch { results.push({ id: containerId, success: false }); }
        }
        setBulkResults(results);
        if (results.every((result) => result.success)) setSelectedDockerIds([]);
      }} /><div className="mb-3">{targetInventoryContent(targetDiscoveryResult ?? targetInventory.data, (containerId, action) => {
        const actionLabels: Record<DockerLifecycleAction, string> = { start: "starten", stop: "stoppen", restart: "neu starten" };
        const containerName = targetInventory.data?.containers.find((container) => container.id === containerId)?.name ?? containerId;
        if (window.confirm(`Docker-Container „${containerName}“ wirklich ${actionLabels[action]}?`)) lifecycle.mutate({ containerId, action });
      }, lifecycle.isPending ? lifecycle.variables?.containerId : undefined, selectedDockerIds, (containerId, selected) => setSelectedDockerIds((current) => selected ? [...new Set([...current, containerId])] : current.filter((id) => id !== containerId)), (containerId) => imageUpdateCheck.mutate(containerId), imageChecks, imageUpdateCheck.isPending ? imageUpdateCheck.variables : undefined, (containerId, expectedRemoteImageId, service) => {
        const container = (targetDiscoveryResult ?? targetInventory.data)?.containers.find((item) => item.id === containerId);
        const project = composeProjectName(container?.labels ?? []) ?? "Compose-Projekt";
        const checked = imageChecks[containerId];
        const currentImage = checked?.current_image_id ?? "aktuellem Image";
        const configHash = container?.labels.find((label) => label.startsWith("com.docker.compose.config-hash="))?.slice("com.docker.compose.config-hash=".length) ?? "Compose-Konfiguration";
        if (window.confirm(`${project} · Änderungsplan für Compose-Service „${service}“\n\nVorher: ${container?.image ?? "Image unbekannt"} (${currentImage})\nNachher: ${container?.image ?? "Image unbekannt"} (${expectedRemoteImageId})\nKonfiguration: ${configHash} wird vor dem Apply erneut geprüft; Compose-Mounts/Volumes, Netzwerke, Secrets und Restart-Policy bleiben aus derselben Compose-Datei erhalten.\n\nAuswirkung: Nur dieser Service wird neu erstellt, andere Services bleiben unberührt. Kurze Unterbrechung möglich. Daten im beschreibbaren Container-Layer gehen beim Ersetzen verloren; persistente Daten gehören in Compose-Mounts.`)) imageUpdateApply.mutate({ containerId, expectedRemoteImageId });
      }, imageUpdateApply.isPending ? imageUpdateApply.variables?.containerId : undefined)} </div>{imageUpdateCheck.error ? <p className="mb-2 text-sm text-[var(--error)]" role="alert">Registry-Updatecheck fehlgeschlagen: {imageUpdateCheck.error.message}</p> : null}{imageUpdateApply.error ? <p className="mb-2 text-sm text-[var(--error)]" role="alert">Compose-Update fehlgeschlagen: {imageUpdateApply.error.message}</p> : null}{imageUpdateApply.isSuccess ? <p className="mb-2 text-sm text-[var(--success)]" role="status">Compose-Service „{imageUpdateApply.data.result.compose_service}“ wurde aktualisiert.</p> : null}<DockerTelemetryPanel discovery={targetDiscoveryResult ?? targetInventory.data} telemetry={targetTelemetry.data} error={targetTelemetry.error} loading={targetTelemetry.isLoading} /></> : containerSelected ? workloadContent(hostId, host?.name, workloads, adopt.isPending, (dockerId) => adopt.mutate(dockerId), (item) => setRemoveCandidate({ id: item.id, name: item.name })) : <SavedTargetDockerInventoryContent targets={lxcTargets} inventories={targetInventories} targetsLoading={targets.isLoading} targetsError={targets.error} />}
    </section>
    <RemoveWorkloadDialog candidate={removeCandidate} pending={remove.isPending} error={remove.error} onCancel={() => setRemoveCandidate(null)} onConfirm={() => removeCandidate && remove.mutate(removeCandidate.id, { onSuccess: () => setRemoveCandidate(null) })} />
  </>;
}

function DockerHostSelector({ targets, containers, value, onSelectTarget, onSelectContainer }: Readonly<{ targets: TargetDto[]; containers: ContainerDto[]; value: string | number; onSelectTarget: (targetId: string) => void; onSelectContainer: (hostId: number | undefined) => void }>) {
  function changeSelection(selection: string) {
    if (selection.startsWith("target:")) {
      onSelectTarget(selection.slice("target:".length));
      return;
    }
    onSelectContainer(selection ? Number(selection) : undefined);
  }
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" id="docker-host-selector"><div className="mb-3"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Docker-Host</p><h2 className="m-0">LXC mit Agent auswählen</h2><p className="mt-1 text-sm text-[var(--muted)]">Docker-Container werden über einen bereits eingebundenen LXC-Agenten erkannt.</p></div><div className="grid max-w-xl grid-cols-1 gap-3"><label className="grid gap-1 text-xs font-semibold text-[var(--muted)]"><span>LXC mit Agent</span><select value={value} onChange={(event) => changeSelection(event.target.value)}><option value="">LXC auswählen</option>{targets.map((target) => <option key={target.id} value={`target:${target.id}`}>{target.name} · {target.address}</option>)}{containers.map((container) => <option key={`container:${container.id}`} value={container.id}>{container.name} · VMID {container.id}</option>)}</select></label></div>{targets.length === 0 ? <p className="mt-2 text-xs text-[var(--muted)]">Es gibt noch kein verbundenes LXC mit gemeldetem Agent-Heartbeat.</p> : null}</section>;
}

function SelectedDockerHost({ targetHost, host, targetDiscoveryPending, discoveryPending, onDiscoverTarget, onDiscoverContainer }: Readonly<{ targetHost: TargetDto | undefined; host: ContainerDto | undefined; targetDiscoveryPending: boolean; discoveryPending: boolean; onDiscoverTarget: () => void; onDiscoverContainer: () => void }>) {
  if (targetHost) return <div className="flex flex-wrap items-center justify-between gap-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Ausgewählter Docker-Host</p><strong>{targetHost.name}</strong><span className="ml-2 text-xs text-[var(--muted)]">LXC · {targetHost.address}</span></div><div className="flex flex-wrap items-center gap-2"><button className="inline-flex items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={targetDiscoveryPending} onClick={onDiscoverTarget}>{targetDiscoveryPending ? "Suche läuft…" : "Docker-Container entdecken"}</button><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={`/targets/${targetHost.id}`}>LXC öffnen</Link></div></div>;
  return <div className="flex flex-wrap items-center justify-between gap-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Ausgewählter Docker-Host</p><strong>{host?.name ?? "Kein LXC mit Agent ausgewählt"}</strong>{host ? <span className="ml-2 text-xs text-[var(--muted)]">LXC · VMID {host.id}</span> : null}</div><div className="flex flex-wrap items-center gap-2"><button className="inline-flex items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={discoveryPending} onClick={onDiscoverContainer}>{discoveryPending ? "Suche läuft…" : "Docker-Container entdecken"}</button>{host ? <Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary" to={`/containers/${host.id}`}>LXC öffnen</Link> : null}</div></div>;
}

function DockerTelemetryPanel({ discovery, telemetry, error, loading }: Readonly<{ discovery: TargetDockerDiscoveryDto | undefined; telemetry: DockerTelemetryDto | undefined; error: Error | null; loading: boolean }>) {
  if (!discovery?.available) return null;
  const samplesByContainer = new Map<string, DockerTelemetryDto["samples"]>();
  for (const sample of telemetry?.samples ?? []) {
    const samples = samplesByContainer.get(sample.container_id) ?? [];
    samples.push(sample);
    samplesByContainer.set(sample.container_id, samples);
  }
  const warnings = discovery.containers.flatMap((container) => {
    const samples = (samplesByContainer.get(container.id) ?? []).slice().sort((a, b) => a.collected_at.localeCompare(b.collected_at));
    const lastThree = samples.slice(-3);
    const latestSample = lastThree.at(-1);
    const fresh = latestSample !== undefined && Date.now() - new Date(latestSample.collected_at).getTime() <= 60_000;
    const sustained = (metric: "cpu_basis_points" | "memory_basis_points") => fresh && lastThree.length === 3 && lastThree.every((sample) => (sample[metric] ?? 0) >= 9_000);
    return [
      sustained("cpu_basis_points") ? `${container.name}: CPU seit mindestens drei Messpunkten über 90 %` : null,
      sustained("memory_basis_points") ? `${container.name}: RAM seit mindestens drei Messpunkten über 90 %` : null,
    ].filter((warning): warning is string => warning !== null);
  });
  return <section className="mt-3 border-t border-[var(--line)] pt-3" aria-labelledby="docker-telemetry-title">
    <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2"><div><h3 id="docker-telemetry-title" className="m-0 text-sm">Container-Auslastung</h3><p className="m-0 text-xs text-[var(--muted)]">CPU und RAM · letzte 10 Minuten · Messung alle 10 Sekunden</p></div><span className="text-xs text-[var(--muted)]">{telemetry?.samples.length ?? 0} Messpunkte</span></div>
    {error ? <p className="text-sm text-[var(--error)]" role="alert">Docker-Telemetrie konnte nicht geladen werden.</p> : null}
    {warnings.length ? <ul className="mb-3 border-l-2 border-[var(--warning)] bg-[var(--warning-soft)] px-3 py-2 text-sm text-[var(--ink)]" role="alert">{warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul> : null}
    {loading ? <p className="text-sm text-[var(--muted)]">Docker-Telemetrie wird geladen…</p> : null}
    {!loading && !error && (telemetry?.samples.length ?? 0) === 0 ? <p className="text-sm text-[var(--muted)]">Noch keine Container-Messwerte. Sie erscheinen nach dem nächsten Agent-Heartbeat.</p> : null}
    {discovery.containers.length > 0 ? <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">{discovery.containers.map((container) => {
      const samples = (samplesByContainer.get(container.id) ?? []).slice().sort((a, b) => a.collected_at.localeCompare(b.collected_at)).slice(-60);
      const latest = samples.at(-1);
      return <article className="border border-[var(--line)] bg-[var(--paper-muted)] p-3" key={container.id}>
        <div className="mb-2 flex items-center justify-between gap-2"><strong className="truncate" title={container.name}>{container.name}</strong><span className="text-xs text-[var(--muted)]">{samples.length} Punkte</span></div>
        <div className="grid grid-cols-2 gap-3"><DockerMetric label="CPU" value={latest?.cpu_basis_points ?? null} samples={samples.map((sample) => sample.cpu_basis_points)} /><DockerMetric label="RAM" value={latest?.memory_basis_points ?? null} samples={samples.map((sample) => sample.memory_basis_points)} /></div>
        {latest?.memory_used_bytes != null && latest.memory_limit_bytes != null ? <p className="mb-0 mt-2 text-xs text-[var(--muted)]">{formatBytes(latest.memory_used_bytes)} / {formatBytes(latest.memory_limit_bytes)}</p> : null}
      </article>;
    })}</div> : null}
  </section>;
}

function DockerMetric({ label, value, samples }: Readonly<{ label: string; value: number | null; samples: Array<number | null> }>) {
  const points = samples.flatMap((sample, index) => sample === null ? [] : [`${samples.length <= 1 ? 160 : (index / (samples.length - 1)) * 320},${48 - Math.min(sample / 100, 100) * 0.44}`]).join(" ");
  return <div><div className="flex justify-between text-xs"><strong>{label}</strong><span>{value === null ? "—" : `${(value / 100).toFixed(1)}%`}</span></div><svg className="mt-1 h-12 w-full" viewBox="0 0 320 52" role="img" aria-label={`${label} Verlauf`}><line x1="0" y1="48" x2="320" y2="48" stroke="currentColor" opacity=".2" /><polyline points={points} fill="none" stroke="var(--primary)" strokeWidth="2" /></svg></div>;
}

function formatBytes(bytes: number) {
  if (bytes < 1_000_000) return `${(bytes / 1_000).toFixed(0)} kB`;
  if (bytes < 1_000_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}

function TargetDockerResult({ discovery, error }: Readonly<{ discovery: TargetDockerDiscoveryDto | undefined; error: Error | null }>) {
  if (error) return <p className="font-semibold text-[var(--error)]" role="alert">Docker-Erkennung fehlgeschlagen: {dockerDiscoveryFailureLabel(error.message)}</p>;
  if (!discovery) return <p className="mt-3 text-sm text-[var(--muted)]">Starte die Erkennung. Der Controller kontaktiert den Agenten im privaten LAN über HTTP.</p>;
  if (discovery.reason === "not_discovered") return <p className="mt-3 text-sm text-[var(--muted)]">Für diesen LXC ist noch kein Docker-Inventar gespeichert. Starte eine Erkennung.</p>;
  if (!discovery.available) return <p className="mt-3 text-sm text-[var(--warning)]">{dockerDiscoveryFailureLabel(discovery.reason ?? "docker_unavailable")} · Host: {discovery.target_name}</p>;
  return <p className="mt-3 text-sm text-[var(--muted)]">Erkannt am {new Date(discovery.collected_at).toLocaleString()} · {discovery.containers.length} Container</p>;
}

function DockerBulkActionBar({ selectedCount, pending, onClear, onSelectAll, onAction }: Readonly<{ selectedCount: number; pending: boolean; onClear: () => void; onSelectAll: () => void; onAction: (action: DockerLifecycleAction) => void }>) {
  return <div className="mb-3 flex flex-wrap items-center justify-between gap-2 border border-[var(--line)] bg-[var(--paper-muted)] p-3"><div className="flex items-center gap-2"><strong className="text-sm">{selectedCount} ausgewählt</strong><button className="border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)]" type="button" onClick={onSelectAll}>Alle auswählen</button><button className="border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)]" type="button" onClick={onClear}>Auswahl leeren</button></div><div className="flex flex-wrap gap-2">{(["start", "stop", "restart"] as const).map((action) => <button key={action} className="border border-[var(--line)] px-2 py-1 text-xs font-semibold hover:border-[var(--primary)] disabled:opacity-50" type="button" disabled={selectedCount === 0 || pending} onClick={() => onAction(action)}>{action === "start" ? "Auswahl starten" : action === "stop" ? "Auswahl stoppen" : "Auswahl neustarten"}</button>)}</div></div>;
}

function targetInventoryContent(discovery: TargetDockerDiscoveryDto | undefined, onAction?: (containerId: string, action: DockerLifecycleAction) => void, pendingContainerId?: string, selectedIds: string[] = [], onSelectionChange?: (containerId: string, selected: boolean) => void, onCheckUpdate?: (containerId: string) => void, imageChecks: Record<string, DockerImageUpdateResult> = {}, pendingImageCheckId?: string, onApplyImageUpdate?: (containerId: string, remoteImageId: string, service: string) => void, pendingImageApplyId?: string) {
  if (!discovery) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Wähle ein verbundenes LXC und starte eine Erkennung.</p>;
  if (discovery.containers.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">{discovery.reason === "not_discovered" ? "Noch kein Docker-Inventar gespeichert." : "Keine Docker-Container vom Agenten gemeldet."}</p>;
  const events = [...(discovery.events ?? [])].reverse();
  return <>
    <div className="overflow-x-auto">
      <table>
        <thead><tr>{onAction ? <th>Auswahl</th> : null}<th>Name</th><th>Compose-Projekt</th><th>Image</th><th>Image-Update</th><th>Ports</th><th>Status</th><th>Health</th><th>Neustarts</th><th>OOM</th><th>Image-ID</th>{onAction ? <th>Aktionen</th> : null}</tr></thead>
        <tbody>
          {discovery.containers.map((item) => (
            <tr key={item.id}>
              {onAction ? <td><input type="checkbox" aria-label={`${item.name} auswählen`} checked={selectedIds.includes(item.id)} onChange={(event) => onSelectionChange?.(item.id, event.target.checked)} /></td> : null}
              <td>
                <strong>{item.name}</strong>
                {item.created_at ? <span className="block text-xs text-[var(--muted)]">Erstellt <time dateTime={item.created_at}>{new Date(item.created_at).toLocaleString()}</time></span> : null}
              </td>
              <td>{composeProjectName(item.labels) ?? "—"}</td>
              <td>{item.image}</td>
              <td>
                {onCheckUpdate ? (() => {
                  const result = imageChecks[item.id];
                  return (
                    <div className="grid gap-1">
                      <button
                        className="w-fit border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)] disabled:opacity-50"
                        type="button"
                        disabled={pendingImageCheckId !== undefined}
                        onClick={() => onCheckUpdate(item.id)}
                      >
                        {pendingImageCheckId === item.id ? "Prüfe…" : "Update prüfen"}
                      </button>
                      {result ? (
                        <span className={result.status === "update_available" ? "text-xs font-semibold text-[var(--warning)]" : "text-xs text-[var(--muted)]"}>
                          {dockerImageUpdateLabel(result.status)}
                          {result.remote_image_id ? ` · ${result.remote_image_id.slice(0, 19)}…` : ""}
                        </span>
                      ) : null}
                      {result?.status === "update_available" && result.remote_image_id && composeUpdateSupported(item.labels) ? <button className="w-fit border border-[var(--warning)] px-2 py-1 text-xs font-semibold text-[var(--warning)] hover:bg-[var(--warning-soft)] disabled:opacity-50" type="button" disabled={pendingImageApplyId !== undefined} onClick={() => onApplyImageUpdate?.(item.id, result.remote_image_id ?? "", composeServiceName(item.labels) ?? "")}>{pendingImageApplyId === item.id ? "Aktualisiere…" : "Compose-Service aktualisieren"}</button> : null}
                    </div>
                  );
                })() : "—"}
              </td>
              <td>{item.ports.length > 0 ? item.ports.join(", ") : "—"}</td>
              <td>{item.state} · {item.status}</td>
              <td>{item.health ?? "Nicht gemeldet"}</td>
              <td>{item.restart_count ?? "—"}</td>
              <td>{item.oom_killed === true ? "Ja" : item.oom_killed === false ? "Nein" : "—"}</td>
              <td title={item.image_id ?? undefined}>{item.image_id ? `${item.image_id.slice(0, 19)}…` : "—"}</td>
              {onAction ? <td><div className="flex flex-wrap gap-1">{item.state === "running" ? <><button className="border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)] disabled:opacity-50" type="button" disabled={pendingContainerId !== undefined} onClick={() => onAction(item.id, "restart")}>{pendingContainerId === item.id ? "Wird ausgeführt…" : "Neustarten"}</button><button className="border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)] disabled:opacity-50" type="button" disabled={pendingContainerId !== undefined} onClick={() => onAction(item.id, "stop")}>Stoppen</button></> : <button className="border border-[var(--line)] px-2 py-1 text-xs hover:border-[var(--primary)] disabled:opacity-50" type="button" disabled={pendingContainerId !== undefined} onClick={() => onAction(item.id, "start")}>{pendingContainerId === item.id ? "Wird ausgeführt…" : "Starten"}</button>}</div></td> : null}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
    {events.length > 0 ? <section className="mt-3 border-t border-[var(--line)] pt-3" aria-label="Docker-Änderungsverlauf">
      <h3 className="mb-2 text-xs font-bold uppercase tracking-wide text-[var(--muted)]">Letzte Änderungen</h3>
      <ul className="m-0 grid list-none gap-1 p-0 text-xs">
        {events.slice(0, 20).map((event, index) => <li className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-[var(--line)] py-1.5 last:border-0" key={`${event.observed_at}-${event.container_id}-${event.kind}-${index}`}>
          <time className="text-[var(--muted)]" dateTime={event.observed_at}>{new Date(event.observed_at).toLocaleString()}</time>
          <strong>{event.container_name}</strong>
          <span>{dockerInventoryEventLabel(event.kind)}</span>
          {event.previous_value || event.current_value ? <span className="text-[var(--muted)]">{event.previous_value ?? "—"} → {event.current_value ?? "—"}</span> : null}
        </li>)}
      </ul>
    </section> : null}
  </>;
}

function dockerInventoryEventLabel(kind: NonNullable<TargetDockerDiscoveryDto["events"]>[number]["kind"]) {
  const labels = {
    added: "hinzugefügt",
    removed: "nicht mehr vorhanden",
    image_changed: "Image geändert",
    state_changed: "Status geändert",
    health_changed: "Health geändert",
    restart_count_increased: "Neustart erkannt",
    oom_killed: "OOM-Kill erkannt",
  };
  return labels[kind];
}

function dockerImageUpdateLabel(status: DockerImageUpdateResult["status"]) {
  const labels = { current: "Aktuell", update_available: "Update verfügbar", pinned: "Digest-fixiert", unknown: "Nicht prüfbar" };
  return labels[status];
}

function SavedTargetDockerInventoryContent({ targets, inventories, targetsLoading, targetsError }: Readonly<{
  targets: TargetDto[];
  inventories: ReturnType<typeof useTargetDockerInventories>;
  targetsLoading: boolean;
  targetsError: Error | null;
}>) {
  const [projectFilter, setProjectFilter] = useState("");
  if (targetsError) {
    return <p className="font-semibold text-[var(--error)]" role="alert">LXC-Ressourcen konnten nicht geladen werden: {targetsError.message}</p>;
  }
  const savedInventories = targets.flatMap((target, index) => {
    const inventory = inventories[index]?.data;
    return inventory?.available ? [{ target, inventory }] : [];
  });
  const loading = targetsLoading || inventories.some((inventory) => inventory.isLoading);
  if (savedInventories.length === 0) {
    if (loading) return <p className="text-[var(--muted)]">Gespeicherte Docker-Inventare werden geladen…</p>;
    if (inventories.some((inventory) => inventory.error)) {
      return <p className="font-semibold text-[var(--error)]" role="alert">Gespeicherte Docker-Inventare konnten nicht geladen werden.</p>;
    }
    return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Docker-Container gespeichert. Wähle einen verbundenen LXC und starte eine Erkennung.</p>;
  }
  const composeProjects = [...new Set(savedInventories.flatMap(({ inventory }) => inventory.containers.map((item) => composeProjectName(item.labels)).filter((project): project is string => project !== null)))].sort((left, right) => left.localeCompare(right));
  const visibleInventories = savedInventories.flatMap(({ target, inventory }) => {
    const groups = new Map<string, typeof inventory.containers>();
    for (const item of inventory.containers) {
      const project = composeProjectName(item.labels) ?? "Ohne Compose-Projekt";
      if (projectFilter && project !== projectFilter) continue;
      groups.set(project, [...(groups.get(project) ?? []), item]);
    }
    return groups.size > 0 ? [{ target, inventory, groups }] : [];
  });
  return (
    <div className="grid gap-3">
      <label className="grid max-w-sm gap-1 text-xs font-semibold text-[var(--muted)]"><span>Nach Compose-Projekt filtern</span><select aria-label="Compose-Projekt filtern" value={projectFilter} onChange={(event) => setProjectFilter(event.target.value)}><option value="">Alle Projekte</option>{composeProjects.map((project) => <option key={project} value={project}>{project}</option>)}</select></label>
      {loading ? <p className="m-0 text-xs text-[var(--muted)]">Weitere Inventare werden geladen…</p> : null}
      {visibleInventories.map(({ target, inventory, groups }) => (
        <article className="border border-[var(--line)] bg-[var(--paper-muted)] p-3" key={target.id}>
          <div className="mb-2 flex flex-wrap items-baseline justify-between gap-2">
            <div>
              <h3 className="m-0 text-sm">{target.name} <span className="font-normal text-[var(--muted)]">· {target.address}</span></h3>
              <Link className="text-xs text-lxcup-primary hover:underline" to={`/docker?target=${encodeURIComponent(target.id)}`}>Erkennung öffnen</Link>
            </div>
            <span className="text-xs text-[var(--muted)]">
              Erkannt am <time dateTime={inventory.collected_at}>{new Date(inventory.collected_at).toLocaleString()}</time> · {Array.from(groups.values()).reduce((count, items) => count + items.length, 0)} Container
            </span>
          </div>
          <div className="grid gap-3">{Array.from(groups.entries()).map(([project, containers]) => {
            const ids = new Set(containers.map((item) => item.id));
            const groupedInventory = { ...inventory, containers, events: (inventory.events ?? []).filter((event) => ids.has(event.container_id)) };
            return <section className="border border-[var(--line)] p-3" key={project}><h4 className="mb-2 text-xs font-bold uppercase tracking-wide text-[var(--muted)]">Compose · {project} <span className="font-normal">· {containers.length} Container</span></h4>{targetInventoryContent(groupedInventory)}</section>;
          })}</div>
        </article>
      ))}
      {visibleInventories.length === 0 && !loading ? <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Für dieses Compose-Projekt sind keine Container gespeichert.</p> : null}
      {inventories.map((inventory, index) => inventory.error ? (
        <p className="m-0 text-sm text-[var(--error)]" role="alert" key={targets[index]?.id ?? index}>
          {targets[index]?.name ?? "LXC"}: Inventar konnte nicht geladen werden.
        </p>
      ) : null)}
    </div>
  );
}

function composeProjectName(labels: string[]) {
  return labels.find((label) => label.startsWith("com.docker.compose.project="))?.slice("com.docker.compose.project=".length) ?? null;
}

function composeServiceName(labels: string[]) {
  return labels.find((label) => label.startsWith("com.docker.compose.service="))?.split("=").slice(1).join("=") || undefined;
}

function composeUpdateSupported(labels: string[]) {
  const required = [
    "com.docker.compose.project=",
    "com.docker.compose.service=",
    "com.docker.compose.project.working_dir=",
    "com.docker.compose.project.config_files=",
    "com.docker.compose.config-hash=",
  ];
  const configFiles = labels.find((label) => label.startsWith(required[3]))?.slice(required[3].length);
  return required.every((prefix) => labels.some((label) => label.startsWith(prefix) && label.length > prefix.length))
    && labels.includes("com.docker.compose.container-number=1")
    && configFiles !== undefined
    && !configFiles.includes(",");
}

function DockerDiscoverySummary({ hostId, hostName, discovery }: Readonly<{ hostId: number | undefined; hostName: string | undefined; discovery: ReturnType<typeof useDockerDiscovery> }>) {
  if (hostId === undefined) return null;
  let result: React.ReactNode;
  if (discovery.isLoading) result = <p className="text-[var(--muted)]">Status wird geladen…</p>;
  else if (discovery.data) result = <DiscoveryRunSummary hostId={hostId} hostName={hostName} run={discovery.data} />;
  else result = <p className="text-[var(--muted)]">Für diesen Host gibt es noch keinen protokollierten Discovery-Lauf.</p>;
  return <div className="border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]" aria-live="polite"><strong>Letzter Discovery-Lauf</strong>{result}{discovery.error ? <p className="font-semibold text-[var(--error)]" role="alert">Discovery-Protokoll konnte nicht geladen werden: {discovery.error.message}</p> : null}</div>;
}

function DiscoveryRunSummary({ hostId, hostName, run }: Readonly<{ hostId: number; hostName: string | undefined; run: NonNullable<ReturnType<typeof useDockerDiscovery>["data"]> }>) {
  return <p>Host: {hostName ?? `LXC ${hostId}`} · Status: <span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", discoveryStatusClass(run.status))}>{run.status}</span>{" · "}{new Date(run.started_at).toLocaleString()} · {run.container_count} Container{run.error_code ? <span className="font-semibold text-[var(--error)]"> · Fehler: {dockerDiscoveryFailureLabel(run.error_code)}</span> : null}</p>;
}

function discoveryStatusClass(status: string) {
  if (status === "succeeded") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (status === "failed") return "inline-block border border-[var(--error)] px-1.5 py-0.5 text-[11px] font-semibold text-[var(--error)]";
  return "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function RemoveWorkloadDialog({ candidate, pending, error, onCancel, onConfirm }: Readonly<{ candidate: { id: string; name: string } | null; pending: boolean; error: Error | null; onCancel: () => void; onConfirm: () => void }>) {
  if (!candidate) return null;
  return <dialog open className="fixed inset-0 z-50 grid place-items-center bg-slate-950/40 p-4" aria-labelledby="remove-title"><div className="w-full max-w-md border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)] shadow-xl"><h2 id="remove-title">Identität entfernen?</h2><p>„{candidate.name}“ wird nur aus dem lxcup-Inventar entfernt. Der Docker-Container läuft unverändert weiter.</p><div className="flex flex-wrap items-center gap-2"><button className="border border-red-300 bg-red-50 px-2 py-1.5 text-sm font-semibold text-red-700 hover:bg-red-100 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={pending} onClick={onConfirm}>{pending ? "Wird entfernt…" : "Ja, Inventareintrag entfernen"}</button><button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onCancel}>Abbrechen</button></div>{error ? <p className="font-semibold text-[var(--error)]" role="alert">{error.message}</p> : null}</div></dialog>;
}

function workloadContent(hostId: number | undefined, hostName: string | undefined, workloads: ReturnType<typeof useDockerWorkloads>, adoptPending: boolean, onAdopt: (id: string) => void, onRemove: (item: { id: string; name: string }) => void) {
  if (hostId === undefined) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Wähle einen LXC und starte eine Erkennung.</p>;
  if (workloads.isLoading) return <p className="text-[var(--muted)]">Docker-Inventar wird geladen…</p>;
  if (workloads.error) return <p className="font-semibold text-[var(--error)]" role="alert">{workloads.error.message}</p>;
  if (workloads.data === undefined || workloads.data.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Docker-Container entdeckt.</p>;
  return <div className="overflow-x-auto"><table><thead><tr><th>Host</th><th>Name</th><th>Image</th><th>Ports</th><th>Status</th><th>Discovery</th><th>Verwaltung</th><th /></tr></thead><tbody>{workloads.data.map((item) => <tr key={item.id}><td>{hostName ?? `LXC ${hostId}`}</td><td><strong>{item.name}</strong><br /><small className="text-[var(--muted)]">{item.id.slice(0, 12)}</small></td><td>{item.image}</td><td>{item.ports.length > 0 ? item.ports.join(", ") : "—"}</td><td>{item.state} · {item.status}<br /><small className="text-[var(--muted)]">{presenceLabel(item.presence)}</small></td><td><time dateTime={item.discovered_at}>{new Date(item.discovered_at).toLocaleString()}</time></td><td><span title={`Abgleich: ${item.change_state}`} className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", workloadStatusClass(item.change_state, item.presence, item.management_state))}>{changeLabel(item.change_state)} · {workloadManagementLabel(item.presence, item.management_state)}</span></td><td><div className="flex flex-wrap items-center gap-2">{item.management_state === "managed" || item.presence === "missing" ? null : <button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={adoptPending} onClick={() => onAdopt(item.id)}>Aufnehmen</button>}<button className="border border-red-300 bg-red-50 px-2 py-1.5 text-sm font-semibold text-red-700 hover:bg-red-100 disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={() => onRemove(item)}>Entfernen</button></div></td></tr>)}</tbody></table></div>;
}

function workloadStatusClass(change: string, presence: string, management: "discovered" | "managed") {
  if (change === "new" || change === "changed") return "bg-[var(--warning-soft)] text-[var(--warning)]";
  if (presence === "missing") return "bg-[var(--paper-muted)] text-[var(--muted)]";
  return management === "managed" ? "bg-[var(--success-soft)] text-[var(--success)]" : "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function presenceLabel(presence: string, management?: "discovered" | "managed") {
  if (presence === "missing") return "Nicht mehr vorhanden";
  if (management) return managementLabel(management);
  return "Aktuell gefunden";
}

function workloadManagementLabel(presence: string, management: "discovered" | "managed") {
  if (presence === "missing") return "nicht mehr vorhanden";
  return managementLabel(management);
}

function managementLabel(state: "discovered" | "managed") {
  return state === "managed" ? "aufgenommen" : "entdeckt";
}

function changeLabel(state: "new" | "changed" | "unchanged" | "missing") {
  return ({ new: "neu", changed: "geändert", unchanged: "unverändert", missing: "verschwunden" } as const)[state];
}
