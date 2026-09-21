import { Link } from "react-router-dom";
import { ResourceSummary } from "../App";
import { useContainers, useNodes } from "../queries";

export function Dashboard() {
  const nodes = useNodes();
  const containers = useContainers();
  return <>
    <header className="page-header dashboard-header"><div><p className="eyebrow">Proxmox Operations / 01</p><h1>CONTROL<br /><span>EVERYTHING.</span></h1><p className="muted">Dein LXC-Bestand, Agenten und Änderungen in einer kompromisslos klaren Oberfläche.</p></div><div className="hero-actions"><Link className="primary-button" to="/enrollments/new">＋ NEUEN LXC AUFNEHMEN</Link><Link className="secondary-button" to="/workflows">WORKFLOWS ÖFFNEN →</Link></div></header>
    <ResourceSummary />
    <section className="dashboard-grid"><ResourcePanel title="PROXMOX-NODES" loading={nodes.isLoading} error={nodes.error} empty={!nodes.isLoading && !nodes.error && nodes.data?.length === 0}><p>{nodes.data?.map((node) => `${node.name} (${node.status})`).join(" · ")}</p><Link className="panel-link" to="/nodes">Nodes öffnen →</Link></ResourcePanel><ResourcePanel title="LXC-INVENTAR" loading={containers.isLoading} error={containers.error} empty={!containers.isLoading && !containers.error && containers.data?.length === 0}><p>{containers.data?.slice(0, 5).map((container) => `${container.name} (${container.status})`).join(" · ")}</p><Link className="panel-link" to="/containers">Inventar öffnen →</Link></ResourcePanel><article className="signal-panel"><span className="signal-icon" aria-hidden="true">↯</span><div><p className="eyebrow">SYSTEM SIGNAL</p><strong>Bereit für das erste Onboarding.</strong><p>Node verbinden, LXC entdecken und Agent kontrolliert ausrollen.</p></div><Link to="/enrollments/new">START →</Link></article></section>
  </>;
}

function ResourcePanel({ title, loading, error, empty, children }: { title: string; loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }) {
  return <article className="panel"><h2>{title}</h2>{loading ? <p className="muted">Daten werden geladen…</p> : error ? <p className="error-state">{error.message}</p> : empty ? <p className="empty-state">Noch keine Daten vorhanden.</p> : children}</article>;
}
