import { createColumnHelper } from "@tanstack/react-table";
import { DataTable } from "../components/DataTable";
import { useNodes } from "../queries";
import type { NodeDto } from "../api";

const column = createColumnHelper<NodeDto>();
const columns = [column.accessor("name", { header: "Name" }), column.accessor("address", { header: "Adresse" }), column.accessor("status", { header: "Status" }), column.accessor("proxmox_version", { header: "Proxmox" })];

export function NodesPage() {
  const nodes = useNodes();
  return <><header className="page-header"><div><p className="eyebrow">Infrastruktur</p><h1>Nodes</h1></div></header><section className="panel">{nodes.isLoading ? <p className="muted">Nodes werden geladen…</p> : nodes.error ? <p className="error-state">{nodes.error.message}</p> : <DataTable columns={columns} data={nodes.data ?? []} emptyMessage="Keine Nodes entdeckt." />}</section></>;
}
