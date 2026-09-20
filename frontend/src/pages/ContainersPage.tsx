import { createColumnHelper } from "@tanstack/react-table";
import { DataTable } from "../components/DataTable";
import { useContainers } from "../queries";
import { useSearchParams } from "react-router-dom";
import { Link } from "react-router-dom";
import type { ContainerDto } from "../api";

const column = createColumnHelper<ContainerDto>();
const columns = [column.accessor("id", { header: "VMID" }), column.accessor("name", { header: "Name", cell: (info) => <Link to={`/containers/${info.row.original.id}`}>{info.getValue()}</Link> }), column.accessor("operating_system", { header: "OS" }), column.accessor("status", { header: "Status" }), column.accessor("management_state", { header: "Verwaltung" })];

export function ContainersPage() {
  const containers = useContainers();
  const [searchParams] = useSearchParams();
  const nodeFilter = searchParams.get("node");
  const visibleContainers = nodeFilter ? (containers.data ?? []).filter((container) => container.node_id === nodeFilter) : (containers.data ?? []);
  return <><header className="page-header"><div><p className="eyebrow">Inventar</p><h1>Container</h1><p className="muted">{nodeFilter ? "Gefiltert nach ausgewähltem Node" : "Alle verwalteten LXCs"}</p></div></header><section className="panel">{containers.isLoading ? <p className="muted">Container werden geladen…</p> : containers.error ? <p className="error-state">{containers.error.message}</p> : <DataTable columns={columns} data={visibleContainers} emptyMessage="Keine Container entdeckt." />}</section></>;
}
