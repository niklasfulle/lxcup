import { cn, ui } from "../ui";
import { Link } from "react-router-dom";
import { ResourceSummary } from "../App";
import { useTargets } from "../queries";

export function Dashboard() {
  const targets = useTargets();
  return <>
    <header className={ui.pageToolbar}><div><div className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Datacenter / lxcup-control</div><h1>Übersicht</h1><p className={ui.muted}>Verwaltung von LXCs, Linux-Servern und Windows-Systemen.</p></div><div className={ui.toolbarActions}><Link className={ui.primaryButton} to="/enrollments/new">＋ LXC einbinden</Link><Link className={ui.button} to="/workflows">Workflows</Link></div></header>
    <ResourceSummary />
    <section className={ui.dashboardGrid}><ResourcePanel title="TARGET-INVENTAR" loading={targets.isLoading} error={targets.error} empty={!targets.isLoading && !targets.error && targets.data?.length === 0}><p>{targets.data?.map((target) => `${target.name} (${target.state})`).join(" · ")}</p><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/targets">Ziele öffnen →</Link></ResourcePanel><ResourcePanel title="TRANSPORTE" loading={targets.isLoading} error={targets.error} empty={!targets.isLoading && !targets.error && targets.data?.length === 0}><p>{targets.data?.map((target) => `${target.transport.toUpperCase()} · ${target.kind}`).join(" · ")}</p><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/targets">Onboarding öffnen →</Link></ResourcePanel><article className={ui.signalPanel}><span className={ui.signalIcon} aria-hidden="true">↯</span><div><p className={ui.eyebrow}>SYSTEM SIGNAL</p><strong>Bereit für das erste Onboarding.</strong><p>Ziel aufnehmen, Agent per Ansible ausrollen und den direkten Heartbeat prüfen.</p></div><Link to="/targets">START →</Link></article></section>
  </>;
}

function ResourcePanel({ title, loading, error, empty, children }: Readonly<{ title: string; loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }>) {
  return <article className={ui.panel}><h2>{title}</h2>{resourcePanelBody({ loading, error, empty, children })}</article>;
}

function resourcePanelBody({ loading, error, empty, children }: Readonly<{ loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }>) {
  if (loading) return <p className={ui.muted}>Daten werden geladen…</p>;
  if (error) return <p className={ui.errorState}>{error.message}</p>;
  if (empty) return <p className={ui.emptyState}>Noch keine Daten vorhanden.</p>;
  return children;
}
