import { Link, useParams } from "react-router-dom";
import { cn, ui } from "../ui";
import { useAnsibleJobs, usePackageInventory, useTargetTelemetry, useTargets } from "../queries";

function stateLabel(state: string) {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Ausstehend";
}

export function TargetDetailPage() {
  const { targetId } = useParams();
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const inventory = usePackageInventory(targetId);
  const telemetry = useTargetTelemetry(targetId);
  const target = targets.data?.find((item) => item.id === targetId);
  const targetJobs = (jobs.data ?? []).filter((job) => "target" in job.target && job.target.target === targetId).slice(0, 8);
  const latest = telemetry.data?.samples.at(-1);
  const stale = latest ? Date.now() - new Date(latest.collected_at).getTime() > 15_000 : false;

  if (targets.isLoading) return <section className={ui.panel}><p className={ui.muted}>Ziel wird geladen…</p></section>;
  if (!target) return <section className={ui.panel}><p className={ui.errorState}>Ziel nicht gefunden.</p><Link className={ui.textLink} to="/targets">Zur Zielübersicht</Link></section>;

  return <>
    <header className={ui.pageHeader}>
      <div><p className={ui.eyebrow}>{target.kind === "lxc" ? "LXC-Container" : target.kind === "windows_server" ? "Windows" : "Server"}</p><h1>{target.name}</h1><p className={ui.muted}>{target.address} · {target.transport.toUpperCase()}</p></div>
      <Link className={ui.button} to={target.kind === "lxc" ? "/containers" : target.kind === "windows_server" ? "/windows" : "/servers"}>← Ressourcenübersicht</Link>
    </header>
    <section className={ui.summaryGrid}>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Lifecycle</span><strong className={cn(ui.statusBadge, target.state === "managed" ? ui.statusSuccess : ui.statusPending)}>{stateLabel(target.state)}</strong></article>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Agent-Version</span><strong>{target.agent_version ?? "Noch nicht gemeldet"}</strong></article>
      <article className={ui.summaryCard}><span className="text-xs text-[var(--muted)]">Letzter Heartbeat</span><strong>{new Date(target.updated_at).toLocaleString()}</strong><small className={ui.muted}>{stale ? "Nicht aktuell" : "Aktuell"}</small></article>
    </section>
    {stale ? <div className={cn(ui.callout, ui.calloutDanger)} role="status">Der letzte Heartbeat ist älter als 15 Sekunden. Telemetrie und Agentstatus können veraltet sein.</div> : null}
    <section className={ui.panelGrid}>
      <article className={ui.panel}><div className={ui.sectionHeading}><div><h2>Paketinventar</h2><p className={ui.muted}>{inventory.isLoading ? "Wird geladen…" : inventory.data?.status === "complete" ? `${inventory.data.packages.length} Pakete` : "Noch nicht erhoben"}</p></div><Link className={ui.button} to={`/targets/${target.id}/packages`}>Inventar öffnen</Link></div>{inventory.error ? <p className={ui.errorState} role="alert">{inventory.error.message}</p> : null}</article>
      <article className={ui.panel}><div className={ui.sectionHeading}><div><h2>Systemauslastung</h2><p className={ui.muted}>{telemetry.isLoading ? "Wird geladen…" : `${telemetry.data?.samples.length ?? 0} Samples im 30-Sekunden-Fenster`}</p></div><Link className={ui.button} to={`/targets/${target.id}/packages`}>Telemetrie öffnen</Link></div>{telemetry.error ? <p className={ui.errorState} role="alert">{telemetry.error.message}</p> : latest ? <p>CPU {latest.cpu_basis_points == null ? "—" : `${(latest.cpu_basis_points / 100).toFixed(1)}%`} · RAM {latest.memory_basis_points == null ? "—" : `${(latest.memory_basis_points / 100).toFixed(1)}%`} · Speicher {latest.storage_basis_points == null ? "—" : `${(latest.storage_basis_points / 100).toFixed(1)}%`}</p> : <p className={ui.emptyState}>Noch keine Telemetrie verfügbar.</p>}</article>
      {target.kind === "lxc" ? <article className={ui.panel}><div className={ui.sectionHeading}><div><h2>Docker-Inventar</h2><p className={ui.muted}>Docker-Container werden getrennt vom LXC-Inventar geführt.</p></div><Link className={ui.button} to="/docker">Docker öffnen</Link></div></article> : null}
    </section>
    <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Letzte Workflows</h2><p className={ui.muted}>Nur Jobs dieses Ziels</p></div><Link className={ui.button} to="/workflows">Alle Workflows</Link></div>{targetJobs.length === 0 ? <p className={ui.emptyState}>Keine Workflows vorhanden.</p> : <ul className={ui.activityList}>{targetJobs.map((job) => <li className="flex items-center justify-between gap-3 border-b border-[var(--line)] py-2 last:border-0" key={job.id}><Link className={ui.textLink} to={`/workflows/${job.id}`}>{job.operation.replaceAll("_", " ")}</Link><span className={cn(ui.statusBadge, job.status === "succeeded" ? ui.statusSuccess : job.status === "failed" ? ui.calloutDanger : ui.statusPending)}>{job.status}</span></li>)}</ul>}</section>
  </>;
}
