import { Link } from "react-router-dom";
import { ResourceSummary } from "../App";
import { useTargets } from "../queries";

export function Dashboard() {
  const targets = useTargets();
  return <>
    <header className="page-toolbar"><div><div className="breadcrumb">Datacenter / lxcup-control</div><h1>Übersicht</h1><p className="muted">Verwaltung von LXCs, Linux-Servern und Windows-Systemen.</p></div><div className="toolbar-actions"><Link className="primary-button" to="/enrollments/new">＋ LXC einbinden</Link><Link className="secondary-button" to="/workflows">Workflows</Link></div></header>
    <ResourceSummary />
    <section className="dashboard-grid"><ResourcePanel title="TARGET-INVENTAR" loading={targets.isLoading} error={targets.error} empty={!targets.isLoading && !targets.error && targets.data?.length === 0}><p>{targets.data?.map((target) => `${target.name} (${target.state})`).join(" · ")}</p><Link className="panel-link" to="/targets">Ziele öffnen →</Link></ResourcePanel><ResourcePanel title="TRANSPORTE" loading={targets.isLoading} error={targets.error} empty={!targets.isLoading && !targets.error && targets.data?.length === 0}><p>{targets.data?.map((target) => `${target.transport.toUpperCase()} · ${target.kind}`).join(" · ")}</p><Link className="panel-link" to="/targets">Onboarding öffnen →</Link></ResourcePanel><article className="signal-panel"><span className="signal-icon" aria-hidden="true">↯</span><div><p className="eyebrow">SYSTEM SIGNAL</p><strong>Bereit für das erste Onboarding.</strong><p>Ziel aufnehmen, Agent per Ansible ausrollen und den direkten Heartbeat prüfen.</p></div><Link to="/targets">START →</Link></article></section>
  </>;
}

function ResourcePanel({ title, loading, error, empty, children }: Readonly<{ title: string; loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }>) {
  return <article className="panel"><h2>{title}</h2>{resourcePanelBody({ loading, error, empty, children })}</article>;
}

function resourcePanelBody({ loading, error, empty, children }: Readonly<{ loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }>) {
  if (loading) return <p className="muted">Daten werden geladen…</p>;
  if (error) return <p className="error-state">{error.message}</p>;
  if (empty) return <p className="empty-state">Noch keine Daten vorhanden.</p>;
  return children;
}
