import { useEffect, useMemo, useState } from "react";
import { Link, NavLink, Route, Routes } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { apiClient, type ApiEvent } from "./api";
import { queryKeys, useContainers, useNodes } from "./queries";
import { DataTable } from "./components/DataTable";
import { Dashboard } from "./pages/Dashboard";
import { NodesPage } from "./pages/NodesPage";
import { ContainersPage } from "./pages/ContainersPage";
import { WorkflowsPage } from "./pages/WorkflowsPage";
import { SecretsPage } from "./pages/SecretsPage";
import { ResourceTree } from "./components/ResourceTree";
import { ContainerDetailPage } from "./pages/ContainerDetailPage";
import { EnrollmentPage } from "./pages/EnrollmentPage";
import { TaskMonitor, type GlobalEvent } from "./components/TaskMonitor";

export default function App() {
  const queryClient = useQueryClient();
  const [connectionState, setConnectionState] = useState("verbunden");
  const [streamError, setStreamError] = useState<string | null>(null);
  const [events, setEvents] = useState<GlobalEvent[]>([]);
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    const stored = globalThis.localStorage?.getItem("lxcup-theme");
    return stored === "light" ? "light" : "dark";
  });

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    globalThis.localStorage?.setItem("lxcup-theme", theme);
  }, [theme]);

  useEffect(() => {
    return apiClient.subscribe(
      (event: ApiEvent) => {
        setConnectionState("verbunden");
        setStreamError(null);
        setEvents((current) => [{ id: crypto.randomUUID(), event, receivedAt: new Date().toISOString() }, ...current].slice(0, 40));
        if (event.type === "Error") setStreamError(event.payload.message);
        if (event.type === "Status") {
          const queryKey = event.payload.resource === "node" ? queryKeys.nodes : event.payload.resource === "ansible_job" ? queryKeys.ansibleJobs : queryKeys.containers;
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
        <p className="brand-caption">LXC / Proxmox Operations</p>
        <nav aria-label="Hauptnavigation">
          <span className="nav-heading">Operate</span>
          <NavLink className="nav-link" to="/"><span aria-hidden="true">▣</span> Übersicht</NavLink>
          <NavLink className="nav-link" to="/nodes"><span aria-hidden="true">⬡</span> Nodes</NavLink>
          <NavLink className="nav-link" to="/containers"><span aria-hidden="true">▤</span> Container</NavLink>
          <span className="nav-heading">Deploy</span>
          <NavLink className="nav-link" to="/enrollments/new"><span aria-hidden="true">＋</span> LXC hinzufügen</NavLink>
          <NavLink className="nav-link" to="/workflows"><span aria-hidden="true">↗</span> Automatisierung</NavLink>
          <NavLink className="nav-link" to="/secrets"><span aria-hidden="true">⌘</span> Secrets</NavLink>
        </nav>
        <ResourceTree />
        <div className="connection-indicator" aria-live="polite">
          <span className={connectionState === "verbunden" ? "status-dot ok" : "status-dot warn"} />
          <span><strong>LIVE BUS</strong><small>SSE: {connectionState}</small></span>
        </div>
      </aside>
      <main className="main-content">
        <header className="topbar">
          <span className="topbar-context"><span className="environment-label">ENV</span><strong>Lokale Entwicklung</strong><span className="muted">/ lxcup-control</span></span>
          <div className="topbar-actions"><Link className="new-lxc-button" to="/enrollments/new">＋ LXC hinzufügen</Link><span className="user-pill">Admin</span><button className="theme-button" type="button" onClick={() => setTheme((current) => current === "dark" ? "light" : "dark")} aria-label="Theme wechseln">{theme === "dark" ? "☼" : "☾"}</button></div>
        </header>
        {streamError ? <div className="api-alert" role="alert">{streamError}</div> : null}
        <TaskMonitor events={events} />
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/nodes" element={<NodesPage />} />
          <Route path="/containers" element={<ContainersPage />} />
          <Route path="/containers/:containerId" element={<ContainerDetailPage />} />
          <Route path="/enrollments/new" element={<EnrollmentPage />} />
          <Route path="/workflows" element={<WorkflowsPage />} />
          <Route path="/secrets" element={<SecretsPage />} />
          <Route path="*" element={<NotFound />} />
        </Routes>
      </main>
    </div>
  );
}

function NotFound() {
  return <section className="panel"><h1>Seite nicht gefunden</h1><Link to="/">Zur Übersicht</Link></section>;
}

export function ResourceSummary() {
  const nodes = useNodes();
  const containers = useContainers();
  const nodeCount = useMemo(() => nodes.data?.length ?? 0, [nodes.data]);
  const containerCount = useMemo(() => containers.data?.length ?? 0, [containers.data]);
  return <div className="summary-grid"><SummaryCard label="Nodes" value={nodeCount} loading={nodes.isLoading} /><SummaryCard label="Container" value={containerCount} loading={containers.isLoading} /></div>;
}

function SummaryCard({ label, value, loading }: { label: string; value: number; loading: boolean }) {
  return <article className="summary-card"><span>{label}</span><strong>{loading ? "…" : value}</strong></article>;
}

export { DataTable };
