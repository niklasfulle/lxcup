import { cn } from "../classnames";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { adoptDockerWorkload, discoverDockerWorkloads, discoverTargetDocker, removeDockerWorkload, type ContainerDto, type TargetDockerDiscoveryDto, type TargetDto } from "../api";
import { queryKeys, useContainers, useDockerDiscovery, useDockerWorkloads, useTargetDockerInventory, useTargets } from "../queries";
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
  const workloads = useDockerWorkloads(hostId);
  const discovery = useDockerDiscovery(hostId);
  const targetInventory = useTargetDockerInventory(targetId);
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
  const adopt = useMutation({ mutationFn: (dockerId: string) => adoptDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const remove = useMutation({ mutationFn: (dockerId: string) => removeDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const [removeCandidate, setRemoveCandidate] = useState<{ id: string; name: string } | null>(null);
  const host = (containers.data ?? []).find((container) => container.id === hostId);
  const targetHost = (targets.data ?? []).find((target) => target.id === targetId);
  const availableLxcTargets = (targets.data ?? []).filter((target) => target.kind === "lxc" && target.state === "managed");
  const targetSelected = Boolean(targetId);
  const containerSelected = Boolean(hostId);

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
      <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Docker-Inventar</h2><p className="text-[var(--muted)]">„Aufnehmen“ verwaltet den Inventareintrag in lxcup; der Docker-Container bleibt unverändert.</p></div><span className="text-[var(--muted)]">{workloads.data?.length ?? 0} Container</span></div>
      {targetSelected ? targetInventoryContent(targetDiscoveryResult ?? targetInventory.data) : workloadContent(hostId, host?.name, workloads, adopt.isPending, (dockerId) => adopt.mutate(dockerId), (item) => setRemoveCandidate({ id: item.id, name: item.name }))}
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

function TargetDockerResult({ discovery, error }: Readonly<{ discovery: TargetDockerDiscoveryDto | undefined; error: Error | null }>) {
  if (error) return <p className="font-semibold text-[var(--error)]" role="alert">Docker-Erkennung fehlgeschlagen: {dockerDiscoveryFailureLabel(error.message)}</p>;
  if (!discovery) return <p className="mt-3 text-sm text-[var(--muted)]">Starte die Erkennung. Der Controller kontaktiert den Agenten im privaten LAN über HTTP.</p>;
  if (discovery.reason === "not_discovered") return <p className="mt-3 text-sm text-[var(--muted)]">Für diesen LXC ist noch kein Docker-Inventar gespeichert. Starte eine Erkennung.</p>;
  if (!discovery.available) return <p className="mt-3 text-sm text-[var(--warning)]">{dockerDiscoveryFailureLabel(discovery.reason ?? "docker_unavailable")} · Host: {discovery.target_name}</p>;
  return <p className="mt-3 text-sm text-[var(--muted)]">Erkannt am {new Date(discovery.collected_at).toLocaleString()} · {discovery.containers.length} Container</p>;
}

function targetInventoryContent(discovery: TargetDockerDiscoveryDto | undefined) {
  if (!discovery) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Wähle ein verbundenes LXC und starte eine Erkennung.</p>;
  if (discovery.containers.length === 0) return <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">{discovery.reason === "not_discovered" ? "Noch kein Docker-Inventar gespeichert." : "Keine Docker-Container vom Agenten gemeldet."}</p>;
  return <div className="overflow-x-auto"><table><thead><tr><th>Name</th><th>Image</th><th>Ports</th><th>Status</th></tr></thead><tbody>{discovery.containers.map((item) => <tr key={item.id}><td><strong>{item.name}</strong></td><td>{item.image}</td><td>{item.ports.length > 0 ? item.ports.join(", ") : "—"}</td><td>{item.state} · {item.status}</td></tr>)}</tbody></table></div>;
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
