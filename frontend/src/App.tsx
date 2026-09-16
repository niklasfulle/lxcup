import { useEffect, useMemo, useState } from "react";
import { Link, NavLink, Route, Routes } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { apiClient, type ApiEvent } from "./api";
import { queryKeys, useContainers, useNodes } from "./queries";
import { DataTable } from "./components/DataTable";
import { Dashboard } from "./pages/Dashboard";
import { NodesPage } from "./pages/NodesPage";
import { ContainersPage } from "./pages/ContainersPage";

export default function App() {
  const queryClient = useQueryClient();
  const [connectionState, setConnectionState] = useState("verbunden");

  useEffect(() => {
    return apiClient.subscribe(
      (event: ApiEvent) => {
        setConnectionState("verbunden");
        if (event.type === "Status") {
          void queryClient.invalidateQueries({ queryKey: event.payload.resource === "node" ? queryKeys.nodes : queryKeys.containers });
        }
      },
      () => setConnectionState("wiederverbinden"),
    );
  }, [queryClient]);

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <Link className="brand" to="/">lxcup</Link>
        <p className="brand-caption">LXC Update Control</p>
        <nav aria-label="Hauptnavigation">
          <NavLink className="nav-link" to="/">Übersicht</NavLink>
          <NavLink className="nav-link" to="/nodes">Nodes</NavLink>
          <NavLink className="nav-link" to="/containers">Container</NavLink>
        </nav>
        <div className="connection-indicator" aria-live="polite">
          <span className={connectionState === "verbunden" ? "status-dot ok" : "status-dot warn"} />
          SSE: {connectionState}
        </div>
      </aside>
      <main className="main-content">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/nodes" element={<NodesPage />} />
          <Route path="/containers" element={<ContainersPage />} />
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
