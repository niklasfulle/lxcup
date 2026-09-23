import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { adoptDockerWorkload, discoverDockerWorkloads, removeDockerWorkload } from "../api";
import { queryKeys, useContainers, useDockerWorkloads } from "../queries";

export function DockerPage() {
  const containers = useContainers();
  const [hostId, setHostId] = useState<number | undefined>();
  const workloads = useDockerWorkloads(hostId);
  const queryClient = useQueryClient();
  const refresh = () => { if (hostId) void queryClient.invalidateQueries({ queryKey: queryKeys.dockerWorkloads(hostId) }); };
  const discover = useMutation({ mutationFn: () => discoverDockerWorkloads(hostId!), onSuccess: refresh });
  const adopt = useMutation({ mutationFn: (dockerId: string) => adoptDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const remove = useMutation({ mutationFn: (dockerId: string) => removeDockerWorkload(hostId!, dockerId), onSuccess: refresh });
  const [removeCandidate, setRemoveCandidate] = useState<{ id: string; name: string } | null>(null);

  return <>
    <header className="page-header"><div><p className="eyebrow">Agent-Inventar</p><h1>Docker-Container</h1><p className="muted">Erkannte Docker-Container auf eingebundenen LXC-Agenten aufnehmen und verwalten.</p></div></header>
    <section className="panel">
      <div className="workflow-grid"><label><span>LXC mit Agent</span>
        <select value={hostId ?? ""} onChange={(event) => setHostId(event.target.value ? Number(event.target.value) : undefined)}>
          <option value="">LXC auswählen</option>
          {(containers.data ?? []).map((container) => <option key={container.id} value={container.id}>{container.name} · VMID {container.id}</option>)}
        </select>
      </label></div>
      <div className="callout info"><strong>Wie die Erkennung funktioniert</strong><p>Der Agent liest ausschließlich Docker-Metadaten: Name, Image und Laufzeitstatus. Es werden keine Umgebungsvariablen, Volumes, Befehle oder Secrets übernommen.</p></div>
      <div className="action-row"><button className="primary-button" type="button" disabled={hostId === undefined || discover.isPending} onClick={() => discover.mutate()}>{discover.isPending ? "Suche läuft…" : "Docker-Container entdecken"}</button>{hostId === undefined ? null : <Link className="theme-button" to={`/containers/${hostId}`}>LXC öffnen</Link>}</div>
      {discover.error ? <p className="error-state" role="alert">{discover.error.message}</p> : null}
    </section>
    <section className="panel">
      <div className="section-heading"><div><h2>Docker-Inventar</h2><p className="muted">„Aufnehmen“ verwaltet den Inventareintrag in lxcup; der Docker-Container bleibt unverändert.</p></div><span className="muted">{workloads.data?.length ?? 0} Container</span></div>
      {workloadContent(hostId, workloads, adopt.isPending, (dockerId) => adopt.mutate(dockerId), (item) => setRemoveCandidate({ id: item.id, name: item.name }))}
    </section>
    {removeCandidate ? <dialog open className="confirm-popover" aria-labelledby="remove-title"><div className="confirm-dialog"><h2 id="remove-title">Identität entfernen?</h2><p>„{removeCandidate.name}“ wird nur aus dem lxcup-Inventar entfernt. Der Docker-Container läuft unverändert weiter.</p><div className="action-row"><button className="danger-button" type="button" disabled={remove.isPending} onClick={() => remove.mutate(removeCandidate.id, { onSuccess: () => setRemoveCandidate(null) })}>{remove.isPending ? "Wird entfernt…" : "Ja, Inventareintrag entfernen"}</button><button className="theme-button" type="button" onClick={() => setRemoveCandidate(null)}>Abbrechen</button></div>{remove.error ? <p className="error-state" role="alert">{remove.error.message}</p> : null}</div></dialog> : null}
  </>;
}

function workloadContent(hostId: number | undefined, workloads: ReturnType<typeof useDockerWorkloads>, adoptPending: boolean, onAdopt: (id: string) => void, onRemove: (item: { id: string; name: string }) => void) {
  if (hostId === undefined) return <p className="empty-state">Wähle einen LXC und starte eine Erkennung.</p>;
  if (workloads.isLoading) return <p className="muted">Docker-Inventar wird geladen…</p>;
  if (workloads.error) return <p className="error-state" role="alert">{workloads.error.message}</p>;
  if (workloads.data === undefined || workloads.data.length === 0) return <p className="empty-state">Noch keine Docker-Container entdeckt.</p>;
  return <div className="table-wrap"><table><thead><tr><th>Name</th><th>Image</th><th>Status</th><th>Verwaltung</th><th /></tr></thead><tbody>{workloads.data.map((item) => <tr key={item.id}><td><strong>{item.name}</strong><br /><small className="muted">{item.id.slice(0, 12)}</small></td><td>{item.image}</td><td>{item.status}</td><td><span className={`status-badge ${managementClass(item.management_state)}`}>{managementLabel(item.management_state)}</span></td><td><div className="action-row">{item.management_state === "managed" ? null : <button className="theme-button" type="button" disabled={adoptPending} onClick={() => onAdopt(item.id)}>Aufnehmen</button>}<button className="danger-button" type="button" onClick={() => onRemove(item)}>Entfernen</button></div></td></tr>)}</tbody></table></div>;
}

function managementClass(state: "discovered" | "managed") {
  return state === "managed" ? "success" : "pending";
}

function managementLabel(state: "discovered" | "managed") {
  return state === "managed" ? "aufgenommen" : "entdeckt";
}
