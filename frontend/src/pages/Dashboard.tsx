import { ResourceSummary } from "../App";
import { useContainers, useNodes } from "../queries";

export function Dashboard() {
  const nodes = useNodes();
  const containers = useContainers();
  return <>
    <header className="page-header"><div><p className="eyebrow">Kontrollzentrum</p><h1>Übersicht</h1><p className="muted">Verwalte Nodes und LXC-Container aus einer Oberfläche.</p></div></header>
    <ResourceSummary />
    <section className="panel-grid"><ResourcePanel title="Proxmox-Nodes" loading={nodes.isLoading} error={nodes.error} empty={!nodes.isLoading && !nodes.error && nodes.data?.length === 0}><p>{nodes.data?.map((node) => `${node.name} (${node.status})`).join(" · ")}</p></ResourcePanel><ResourcePanel title="LXC-Container" loading={containers.isLoading} error={containers.error} empty={!containers.isLoading && !containers.error && containers.data?.length === 0}><p>{containers.data?.slice(0, 5).map((container) => `${container.name} (${container.status})`).join(" · ")}</p></ResourcePanel></section>
  </>;
}

function ResourcePanel({ title, loading, error, empty, children }: { title: string; loading: boolean; error: Error | null; empty: boolean; children: React.ReactNode }) {
  return <article className="panel"><h2>{title}</h2>{loading ? <p className="muted">Daten werden geladen…</p> : error ? <p className="error-state">{error.message}</p> : empty ? <p className="empty-state">Noch keine Daten vorhanden.</p> : children}</article>;
}
