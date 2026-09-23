import { createColumnHelper } from "@tanstack/react-table";
import { DataTable } from "../components/DataTable";
import { useContainers } from "../queries";
import { Link, useSearchParams } from "react-router-dom";
import type { ContainerDto } from "../api";

const column = createColumnHelper<ContainerDto>();
const columns = [column.accessor("id", { header: "VMID" }), column.accessor("name", { header: "Name", cell: (info) => <Link to={`/containers/${info.row.original.id}`}>{info.getValue()}</Link> }), column.accessor("operating_system", { header: "OS" }), column.accessor("status", { header: "Status" }), column.accessor("management_state", { header: "Verwaltung" })];

export function ContainersPage() {
  const containers = useContainers();
  const [searchParams] = useSearchParams();
  const nodeFilter = searchParams.get("node");
  const visibleContainers = nodeFilter ? (containers.data ?? []).filter((container) => container.node_id === nodeFilter) : (containers.data ?? []);
  return <><header className="page-header"><div><p className="eyebrow">LXC-Container</p><h1>LXC-Inventar</h1><p className="muted">{nodeFilter ? "Gefiltert nach ausgewähltem Node" : "Entdeckte LXC-Container aus dem Infrastrukturinventar."}</p></div><Link className="secondary-button" to="/enrollments/new">LXC aufnehmen</Link></header><div className="callout info inventory-callout"><strong>Was erscheint hier?</strong><p>Nur LXC-Container, die über einen Node entdeckt wurden. Ein Zugangsprofil allein erzeugt keinen Inventareintrag. Verbinde einen entdeckten LXC über „LXC aufnehmen“ mit seinem Zugang.</p></div><section className="panel">{containerBody(containers, visibleContainers)}</section></>;
}

function containerBody(containers: ReturnType<typeof useContainers>, visibleContainers: ContainerDto[]) {
  if (containers.isLoading) return <p className="muted">Container werden geladen…</p>;
  if (containers.error) return <p className="error-state">{containers.error.message}</p>;
  return <DataTable columns={columns} data={visibleContainers} emptyMessage="Keine LXC-Container entdeckt. Prüfe zuerst die Node-Inventarisierung." />;
}
