import { createColumnHelper } from "@tanstack/react-table";
import { DataTable } from "../components/DataTable";
import { useContainers } from "../queries";
import type { ContainerDto } from "../api";

const column = createColumnHelper<ContainerDto>();
const columns = [column.accessor("id", { header: "VMID" }), column.accessor("name", { header: "Name" }), column.accessor("operating_system", { header: "OS" }), column.accessor("status", { header: "Status" }), column.accessor("management_state", { header: "Verwaltung" })];

export function ContainersPage() {
  const containers = useContainers();
  return <><header className="page-header"><div><p className="eyebrow">Inventar</p><h1>Container</h1></div></header><section className="panel">{containers.isLoading ? <p className="muted">Container werden geladen…</p> : containers.error ? <p className="error-state">{containers.error.message}</p> : <DataTable columns={columns} data={containers.data ?? []} emptyMessage="Keine Container entdeckt." />}</section></>;
}
