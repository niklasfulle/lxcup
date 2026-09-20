import { Link } from "react-router-dom";
import { useContainers, useNodes } from "../queries";

export function ResourceTree() {
  const nodes = useNodes();
  const containers = useContainers();
  const items = containers.data ?? [];
  return <section className="resource-tree" aria-label="Ressourcenbaum">
    <div className="tree-heading"><span>Ressourcen</span><span className="muted">{items.length}</span></div>
    <Link className="tree-root" to="/nodes">Proxmox Nodes</Link>
    {nodes.data?.map((node) => <details className="tree-node" key={node.id} open>
      <summary><span className="status-dot ok" />{node.name}<span className="muted">{items.filter((item) => item.node_id === node.id).length}</span></summary>
      <div className="tree-children">
        {items.filter((item) => item.node_id === node.id).map((container) => <Link key={container.id} to={`/containers?node=${encodeURIComponent(node.id)}`}>↳ {container.name} <span className="muted">{container.id}</span></Link>)}
        {!items.some((item) => item.node_id === node.id) ? <span className="muted">Keine LXCs</span> : null}
      </div>
    </details>)}
    {!nodes.data?.length && !nodes.isLoading ? <span className="muted tree-empty">Keine Nodes verbunden</span> : null}
  </section>;
}
