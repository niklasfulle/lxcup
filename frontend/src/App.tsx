import { useEffect, useMemo, useState } from "react";
import { Link, NavLink, Route, Routes } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { apiClient, type ApiEvent } from "./api";
import { queryKeys, useAnsibleJobs, useTargets } from "./queries";
import { Dashboard } from "./pages/Dashboard";
import { WorkflowsPage } from "./pages/WorkflowsPage";
import { WorkflowDetailPage } from "./pages/WorkflowDetailPage";
import { SecretsPage } from "./pages/SecretsPage";
import { ContainerDetailPage } from "./pages/ContainerDetailPage";
import { EnrollmentPage } from "./pages/EnrollmentPage";
import { TargetsPage } from "./pages/TargetsPage";
import { DockerPage } from "./pages/DockerPage";
import { TaskMonitor, type GlobalEvent } from "./components/TaskMonitor";

export default function App() {
  const queryClient = useQueryClient();
  const [connectionState, setConnectionState] = useState("verbunden");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [events, setEvents] = useState<GlobalEvent[]>([]);
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    const stored = globalThis.localStorage?.getItem("lxcup-theme-v2");
    return stored === "dark" ? "dark" : "light";
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    globalThis.localStorage?.setItem("lxcup-theme-v2", theme);
  }, [theme]);

  useEffect(() => {
    return apiClient.subscribe(
      (event: ApiEvent) => {
        setConnectionState("verbunden");
        setStreamError(null);
        setEvents((current) => [{ id: crypto.randomUUID(), event, receivedAt: new Date().toISOString() }, ...current].slice(0, 40));
        if (event.type === "Error") setStreamError(event.payload.message);
        if (event.type === "Status") {
          const queryKey = eventQueryKey(event.payload.resource);
          void queryClient.invalidateQueries({ queryKey });
        }
      },
      () => { setConnectionState("wiederverbinden"); setStreamError("Echtzeitverbindung unterbrochen. Der Browser versucht die Verbindung erneut."); },
    );
  }, [queryClient]);

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <Link className="brand" to="/" aria-label="lxcup Übersicht">
          <span className="brand-mark" aria-hidden="true">LX</span>
          <span><strong>lxcup</strong><small>CONTROL PLANE</small></span>
        </Link>
        <p className="brand-caption">Datacenter Control</p>
        <nav aria-label="Hauptnavigation">
          <span className="nav-heading">Übersicht</span>
          <NavLink className="nav-link" to="/"><span aria-hidden="true">▣</span> Übersicht</NavLink>
          <span className="nav-heading">Ressourcen</span>
          <NavLink className="nav-link" to="/servers"><span aria-hidden="true">▰</span> Server</NavLink>
          <NavLink className="nav-link" to="/containers"><span aria-hidden="true">▤</span> LXC-Container</NavLink>
          <NavLink className="nav-link" to="/docker"><span aria-hidden="true">◫</span> Docker-Container</NavLink>
          <NavLink className="nav-link" to="/windows"><span aria-hidden="true">▣</span> Windows</NavLink>
          <span className="nav-heading">Automatisierung</span>
          <NavLink className="nav-link" to="/workflows"><span aria-hidden="true">↗</span> Tasks & Workflows</NavLink>
          <span className="nav-heading">System</span>
          <NavLink className="nav-link" to="/secrets"><span aria-hidden="true">⌘</span> Secrets</NavLink>
        </nav>
        <div className="connection-indicator" aria-live="polite">
          <span className={connectionState === "verbunden" ? "status-dot ok" : "status-dot warn"} />
          <span><strong>LIVE BUS</strong><small>SSE: {connectionState}</small></span>
        </div>
      </aside>
      <main className="main-content">
        <header className="topbar">
          <div className="topbar-context"><span className="environment-label">Datacenter</span><strong>lxcup-control</strong><span className="muted">/ Übersicht</span></div>
          <div className="topbar-actions"><NotificationCenter /><span className="user-pill">Admin</span><button className="theme-button" type="button" onClick={() => setTheme((current) => current === "dark" ? "light" : "dark")} aria-label="Theme wechseln">{theme === "dark" ? "☼" : "☾"}</button></div>
        </header>
        <div className="content-area">
          {streamError ? <div className="api-alert" role="alert">{streamError}</div> : null}
          <TaskMonitor events={events} onClear={() => setEvents([])} />
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/servers" element={<TargetsPage area="linux_server" />} />
            <Route path="/windows" element={<TargetsPage area="windows_server" />} />
            <Route path="/containers" element={<TargetsPage area="lxc" />} />
            <Route path="/targets" element={<TargetsPage />} />
            <Route path="/containers/:containerId" element={<ContainerDetailPage />} />
            <Route path="/docker" element={<DockerPage />} />
            <Route path="/enrollments/new" element={<EnrollmentPage />} />
            <Route path="/workflows" element={<WorkflowsPage />} />
            <Route path="/workflows/:jobId" element={<WorkflowDetailPage />} />
            <Route path="/secrets" element={<SecretsPage />} />
            <Route path="*" element={<NotFound />} />
          </Routes>
        </div>
      </main>
    </div>
  );
}

function NotificationCenter() {
  const jobs = useAnsibleJobs();
  const failed = (jobs.data ?? []).filter((job) => job.status === "failed" || job.status === "reconcile_required");
  const [open, setOpen] = useState(false);
  const [read, setRead] = useState<string[]>(() => JSON.parse(globalThis.localStorage?.getItem("lxcup-read-notifications") ?? "[]") as string[]);
  const unread = failed.filter((job) => read.includes(job.id) === false);
  const markRead = (id: string) => setRead((current) => { const next = current.includes(id) ? current : [...current, id]; globalThis.localStorage?.setItem("lxcup-read-notifications", JSON.stringify(next)); return next; });
  return <div className="notification-center"><button className="theme-button notification-button" type="button" aria-label="Benachrichtigungen" onClick={() => setOpen((value) => value === false)}>🔔{unread.length ? <span className="notification-count">{unread.length}</span> : null}</button>{open ? <div className="notification-popover"><strong>Benachrichtigungen</strong>{notificationBody(failed, markRead)}</div> : null}</div>;
}

function eventQueryKey(resource: string) {
  if (resource === "target") return queryKeys.targets;
  if (resource === "ansible_job") return queryKeys.ansibleJobs;
  return queryKeys.containers;
}

function notificationBody(failed: ReturnType<typeof useAnsibleJobs>["data"], markRead: (id: string) => void) {
  if (failed === undefined || failed.length === 0) return <p className="muted">Keine fehlgeschlagenen Jobs.</p>;
  return failed.slice(0, 8).map((job) => <Link key={job.id} to={`/workflows/${job.id}`} onClick={() => markRead(job.id)}><span className={`status-badge ${job.status === "failed" ? "neutral" : "pending"}`}>{job.status === "failed" ? "Fehlgeschlagen" : "Abgleich"}</span><span>{job.operation.replaceAll("_", " ")}</span><small>{new Date(job.updated_at).toLocaleString()}</small></Link>);
}

function NotFound() {
  return <section className="panel"><h1>Seite nicht gefunden</h1><Link to="/">Zur Übersicht</Link></section>;
}

export function ResourceSummary() {
  const targets = useTargets();
  const targetCount = useMemo(() => targets.data?.length ?? 0, [targets.data]);
  const connectedCount = useMemo(() => targets.data?.filter((target) => target.state === "managed").length ?? 0, [targets.data]);
  return <div className="summary-grid"><SummaryCard label="Verwaltete Ressourcen" value={targetCount} loading={targets.isLoading} /><SummaryCard label="Agenten verbunden" value={connectedCount} loading={targets.isLoading} /></div>;
}

function SummaryCard({ label, value, loading }: Readonly<{ label: string; value: number; loading: boolean }>) {
  return <article className="summary-card"><span>{label}</span><strong>{loading ? "…" : value}</strong></article>;
}

export { DataTable } from "./components/DataTable";
