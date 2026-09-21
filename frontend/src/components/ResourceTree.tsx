import { Link } from "react-router-dom";
import { useTargets } from "../queries";

export function ResourceTree() {
  const targets = useTargets();
  const items = targets.data ?? [];
  return <section className="resource-tree" aria-label="Ressourcenbaum">
    <div className="tree-heading"><span>Ressourcen</span><span className="muted">{items.length}</span></div>
    <Link className="tree-root" to="/targets">Verwaltete Ziele</Link>
    {items.map((target) => <Link className="tree-root" key={target.id} to="/targets"><span className={`status-dot ${target.state === "managed" ? "ok" : "warn"}`} />{target.name}<span className="muted">{target.kind}</span></Link>)}
    {!items.length && !targets.isLoading ? <span className="muted tree-empty">Keine Ziele angelegt</span> : null}
  </section>;
}
