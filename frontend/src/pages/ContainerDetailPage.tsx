import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router-dom";
import { createContainerAction, getAgentHealth, getAgentMetrics, type AgentHealthDto, type ContainerAction } from "../api";
import { useContainers } from "../queries";

const actions: Array<{ value: ContainerAction; label: string }> = [
  { value: "start", label: "Start" },
  { value: "shutdown", label: "Shutdown" },
  { value: "reboot", label: "Reboot" },
  { value: "refresh", label: "Refresh" },
];

export function ContainerDetailPage() {
  const { containerId } = useParams();
  const id = Number(containerId);
  const containers = useContainers();
  const container = containers.data?.find((item) => item.id === id);
  const health = useQuery({ queryKey: ["agent-health", id], queryFn: ({ signal }) => getAgentHealth(id, signal), enabled: Boolean(container) });
  const metrics = useQuery({ queryKey: ["agent-metrics", id], queryFn: ({ signal }) => getAgentMetrics(id, signal), enabled: Boolean(container) });
  const action = useMutation({ mutationFn: (value: ContainerAction) => createContainerAction(id, value, true) });

  if (!container) return <section className="panel"><h1>LXC nicht gefunden</h1><p className="muted">Der Container ist nicht im aktuellen Inventar.</p><Link to="/containers">Zurück zum Inventar</Link></section>;

  const agentContent = renderAgentContent(health.isLoading, health.data);
  return <>
    <header className="page-header"><div><p className="eyebrow">LXC · VMID {container.id}</p><h1>{container.name}</h1><p className="muted">{container.operating_system} · Node {container.node_id}</p></div><Link className="theme-button" to="/containers">← Inventar</Link></header>
    <div className="panel-grid">
      <section className="panel"><h2>Zusammenfassung</h2><dl className="detail-list"><dt>Status</dt><dd>{container.status}</dd><dt>Verwaltung</dt><dd>{container.management_state}</dd><dt>Entdeckt</dt><dd>{new Date(container.discovered_at).toLocaleString()}</dd></dl></section>
      <section className="panel"><h2>Agent</h2>{agentContent}</section>
    </div>
    <section className="panel"><div className="section-heading"><h2>Aktionen</h2><span className="muted">Jede Aktion erzeugt einen Task</span></div><div className="action-row">{actions.map((item) => <button key={item.value} type="button" className="theme-button" disabled={action.isPending || (item.value === "start" && container.status === "running") || (item.value !== "start" && item.value !== "refresh" && container.status !== "running")} onClick={() => { if (globalThis.confirm(`Aktion ${item.label} für ${container.name} bestätigen?`)) action.mutate(item.value); }}>{item.label}</button>)}</div>{action.data ? <output className="success-state">Task {action.data.id} angenommen · {action.data.status}</output> : null}{action.error ? <p className="error-state" role="alert">{action.error.message}</p> : null}</section>
    <div className="panel-grid"><section className="panel"><h2>Metriken</h2>{metrics.data ? <dl className="detail-list"><dt>Befehle</dt><dd>{metrics.data.commands_total}</dd><dt>Fehler</dt><dd>{metrics.data.commands_failed}</dd><dt>Letzter Befehl</dt><dd>{metrics.data.last_command_at ? new Date(metrics.data.last_command_at).toLocaleString() : "—"}</dd></dl> : <p className="muted">Keine Agent-Metriken verfügbar.</p>}</section><section className="panel"><h2>Konfiguration</h2><p className="muted">Änderungen werden über validierte Ansible-Workflows geplant und bestätigt.</p><Link className="theme-button" to="/workflows">Workflow öffnen</Link></section></div>
  </>;
}

function renderAgentContent(loading: boolean, data: AgentHealthDto | undefined) {
  if (loading) return <p className="muted">Health wird geladen…</p>;
  if (data === undefined) return <p className="muted">Agent nicht verbunden.</p>;
  return <dl className="detail-list"><dt>Verbindung</dt><dd>{data.healthy ? "Healthy" : "Fehler"}</dd><dt>Version</dt><dd>{data.info.version}</dd><dt>Plattform</dt><dd>{data.info.platform}</dd><dt>Hostname</dt><dd>{data.info.hostname}</dd></dl>;
}
