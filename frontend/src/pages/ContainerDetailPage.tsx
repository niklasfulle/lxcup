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

  if (!container) return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h1>LXC nicht gefunden</h1><p className="text-[var(--muted)]">Der Container ist nicht im aktuellen Inventar.</p><Link to="/containers">Zurück zum Inventar</Link></section>;

  const agentContent = renderAgentContent(health.isLoading, health.data);
  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">LXC · VMID {container.id}</p><h1>{container.name}</h1><p className="text-[var(--muted)]">{container.operating_system} · Node {container.node_id}</p></div><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to="/containers">← Inventar</Link></header>
    <div className="mb-3 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h2>Zusammenfassung</h2><dl className="grid grid-cols-[minmax(8rem,.7fr)_1fr] gap-x-4 gap-y-2"><dt>Status</dt><dd>{container.status}</dd><dt>Verwaltung</dt><dd>{container.management_state}</dd><dt>Entdeckt</dt><dd>{new Date(container.discovered_at).toLocaleString()}</dd></dl></section>
      <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h2>Agent</h2>{agentContent}</section>
    </div>
    <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><h2>Aktionen</h2><span className="text-[var(--muted)]">Jede Aktion erzeugt einen Task</span></div><div className="flex flex-wrap items-center gap-2">{actions.map((item) => <button key={item.value} type="button" className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" disabled={action.isPending || (item.value === "start" && container.status === "running") || (item.value !== "start" && item.value !== "refresh" && container.status !== "running")} onClick={() => { if (globalThis.confirm(`Aktion ${item.label} für ${container.name} bestätigen?`)) action.mutate(item.value); }}>{item.label}</button>)}</div>{action.data ? <output className="border border-[#b7e7d0] bg-[var(--success-soft)] px-2 py-1.5 font-medium text-[var(--success)]">Task {action.data.id} angenommen · {action.data.status}</output> : null}{action.error ? <p className="font-semibold text-[var(--error)]" role="alert">{action.error.message}</p> : null}</section>
    <div className="mb-3 grid grid-cols-1 gap-3 lg:grid-cols-2"><section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h2>Metriken</h2>{metrics.data ? <dl className="grid grid-cols-[minmax(8rem,.7fr)_1fr] gap-x-4 gap-y-2"><dt>Befehle</dt><dd>{metrics.data.commands_total}</dd><dt>Fehler</dt><dd>{metrics.data.commands_failed}</dd><dt>Letzter Befehl</dt><dd>{metrics.data.last_command_at ? new Date(metrics.data.last_command_at).toLocaleString() : "—"}</dd></dl> : <p className="text-[var(--muted)]">Keine Agent-Metriken verfügbar.</p>}</section><section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h2>Konfiguration</h2><p className="text-[var(--muted)]">Änderungen werden über validierte Ansible-Workflows geplant und bestätigt.</p><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to="/workflows">Workflow öffnen</Link></section></div>
  </>;
}

function renderAgentContent(loading: boolean, data: AgentHealthDto | undefined) {
  if (loading) return <p className="text-[var(--muted)]">Health wird geladen…</p>;
  if (data === undefined) return <p className="text-[var(--muted)]">Agent nicht verbunden.</p>;
  return <dl className="grid grid-cols-[minmax(8rem,.7fr)_1fr] gap-x-4 gap-y-2"><dt>Verbindung</dt><dd>{data.healthy ? "Healthy" : "Fehler"}</dd><dt>Version</dt><dd>{data.info.version}</dd><dt>Plattform</dt><dd>{data.info.platform}</dd><dt>Hostname</dt><dd>{data.info.hostname}</dd></dl>;
}
