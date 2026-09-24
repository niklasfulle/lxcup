import { cn, ui } from "../ui";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { adoptDockerWorkload, discoverDockerWorkloads, removeDockerWorkload } from "../api";
import { queryKeys, useContainers, useDockerDiscovery, useDockerWorkloads } from "../queries";

export function DockerPage() {
  const containers = useContainers();
  const [hostId, setHostId] = useState<number | undefined>();
  const workloads = useDockerWorkloads(hostId);
  const discovery = useDockerDiscovery(hostId);
  const queryClient = useQueryClient();
  const refresh = () => {
    if (hostId) {
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerWorkloads(hostId) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerDiscovery(hostId) });
    }
  };
  const discover = useMutation({ mutationFn: () => discoverDockerWorkloads(hostId!), onSuccess: refresh });
  const adopt = useMutation({ mutationFn: (dockerId: string) => adoptDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const remove = useMutation({ mutationFn: (dockerId: string) => removeDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const [removeCandidate, setRemoveCandidate] = useState<{ id: string; name: string } | null>(null);
  const host = (containers.data ?? []).find((container) => container.id === hostId);

  return <>
    <header className={ui.pageHeader}><div><p className={ui.eyebrow}>Agent-Inventar</p><h1>Docker-Container</h1><p className={ui.muted}>Erkannte Docker-Container auf eingebundenen LXC-Agenten aufnehmen und verwalten.</p></div></header>
    <section className={ui.panel}>
      <div className={ui.workflowGrid}><label><span>LXC mit Agent</span>
        <select value={hostId ?? ""} onChange={(event) => setHostId(event.target.value ? Number(event.target.value) : undefined)}>
          <option value="">LXC auswählen</option>
          {(containers.data ?? []).map((container) => <option key={container.id} value={container.id}>{container.name} · VMID {container.id}</option>)}
        </select>
      </label></div>
      <div className={cn(ui.callout, ui.calloutInfo)}><strong>Wie die Erkennung funktioniert</strong><p>Der Agent liest ausschließlich Docker-Metadaten: Name, Image und Laufzeitstatus. Es werden keine Umgebungsvariablen, Volumes, Befehle oder Secrets übernommen.</p></div>
      <div className={ui.actionRow}><button className={ui.primaryButton} type="button" disabled={hostId === undefined || discover.isPending} onClick={() => discover.mutate()}>{discover.isPending ? "Suche läuft…" : "Docker-Container entdecken"}</button>{hostId === undefined ? null : <Link className={ui.button} to={`/containers/${hostId}`}>LXC öffnen</Link>}</div>
      {discover.error ? <p className={ui.errorState} role="alert">{discover.error.message}</p> : null}
      {hostId === undefined ? null : <div className={ui.callout} aria-live="polite">
        <strong>Letzter Discovery-Lauf</strong>
        {discovery.isLoading ? <p className={ui.muted}>Status wird geladen…</p> : discovery.data ? <p>
          Host: {host?.name ?? `LXC ${hostId}`} · Status: <span className={cn(ui.statusBadge, discovery.data.status === "succeeded" ? ui.statusSuccess : discovery.data.status === "failed" ? ui.failureCode : ui.statusPending)}>{discovery.data.status}</span>
          {" · "}{new Date(discovery.data.started_at).toLocaleString()} · {discovery.data.container_count} Container
          {discovery.data.error_code ? <span className={ui.errorState}> · Fehler: {discovery.data.error_code}</span> : null}
        </p> : <p className={ui.muted}>Für diesen Host gibt es noch keinen protokollierten Discovery-Lauf.</p>}
        {discovery.error ? <p className={ui.errorState} role="alert">Discovery-Protokoll konnte nicht geladen werden: {discovery.error.message}</p> : null}
      </div>}
    </section>
    <section className={ui.panel}>
      <div className={ui.sectionHeading}><div><h2>Docker-Inventar</h2><p className={ui.muted}>„Aufnehmen“ verwaltet den Inventareintrag in lxcup; der Docker-Container bleibt unverändert.</p></div><span className={ui.muted}>{workloads.data?.length ?? 0} Container</span></div>
      {workloadContent(hostId, host?.name, workloads, adopt.isPending, (dockerId) => adopt.mutate(dockerId), (item) => setRemoveCandidate({ id: item.id, name: item.name }))}
    </section>
    {removeCandidate ? <dialog open className={ui.confirmPopover} aria-labelledby="remove-title"><div className={ui.confirmDialog}><h2 id="remove-title">Identität entfernen?</h2><p>„{removeCandidate.name}“ wird nur aus dem lxcup-Inventar entfernt. Der Docker-Container läuft unverändert weiter.</p><div className={ui.actionRow}><button className={ui.dangerButton} type="button" disabled={remove.isPending} onClick={() => remove.mutate(removeCandidate.id, { onSuccess: () => setRemoveCandidate(null) })}>{remove.isPending ? "Wird entfernt…" : "Ja, Inventareintrag entfernen"}</button><button className={ui.button} type="button" onClick={() => setRemoveCandidate(null)}>Abbrechen</button></div>{remove.error ? <p className={ui.errorState} role="alert">{remove.error.message}</p> : null}</div></dialog> : null}
  </>;
}

function workloadContent(hostId: number | undefined, hostName: string | undefined, workloads: ReturnType<typeof useDockerWorkloads>, adoptPending: boolean, onAdopt: (id: string) => void, onRemove: (item: { id: string; name: string }) => void) {
  if (hostId === undefined) return <p className={ui.emptyState}>Wähle einen LXC und starte eine Erkennung.</p>;
  if (workloads.isLoading) return <p className={ui.muted}>Docker-Inventar wird geladen…</p>;
  if (workloads.error) return <p className={ui.errorState} role="alert">{workloads.error.message}</p>;
  if (workloads.data === undefined || workloads.data.length === 0) return <p className={ui.emptyState}>Noch keine Docker-Container entdeckt.</p>;
  return <div className={ui.tableWrap}><table><thead><tr><th>Host</th><th>Name</th><th>Image</th><th>Ports</th><th>Status</th><th>Discovery</th><th>Verwaltung</th><th /></tr></thead><tbody>{workloads.data.map((item) => <tr key={item.id}><td>{hostName ?? `LXC ${hostId}`}</td><td><strong>{item.name}</strong><br /><small className={ui.muted}>{item.id.slice(0, 12)}</small></td><td>{item.image}</td><td>{item.ports.length ? item.ports.join(", ") : "—"}</td><td>{item.state} · {item.status}<br /><small className={ui.muted}>{item.presence === "missing" ? "Nicht mehr gefunden" : "Aktuell gefunden"}</small></td><td><time dateTime={item.discovered_at}>{new Date(item.discovered_at).toLocaleString()}</time></td><td><span title={`Abgleich: ${item.change_state}`} className={cn(ui.statusBadge, item.change_state === "new" || item.change_state === "changed" ? ui.statusPending : item.change_state === "missing" ? ui.statusNeutral : managementClass(item.management_state) === "success" ? ui.statusSuccess : ui.statusPending)}>{changeLabel(item.change_state)} · {item.presence === "missing" ? "nicht mehr vorhanden" : managementLabel(item.management_state)}</span></td><td><div className={ui.actionRow}>{item.management_state === "managed" || item.presence === "missing" ? null : <button className={ui.button} type="button" disabled={adoptPending} onClick={() => onAdopt(item.id)}>Aufnehmen</button>}<button className={ui.dangerButton} type="button" onClick={() => onRemove(item)}>Entfernen</button></div></td></tr>)}</tbody></table></div>;
}

function managementClass(state: "discovered" | "managed") {
  return state === "managed" ? "success" : "pending";
}

function managementLabel(state: "discovered" | "managed") {
  return state === "managed" ? "aufgenommen" : "entdeckt";
}

function changeLabel(state: "new" | "changed" | "unchanged" | "missing") {
  return ({ new: "neu", changed: "geändert", unchanged: "unverändert", missing: "verschwunden" } as const)[state];
}
