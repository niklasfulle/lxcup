import { cn, ui } from "../ui";
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
  return <><header className={ui.pageHeader}><div><p className={ui.eyebrow}>LXC-Container</p><h1>LXC-Inventar</h1><p className={ui.muted}>{nodeFilter ? "Gefiltert nach ausgewähltem Node" : "Entdeckte LXC-Container aus dem Infrastrukturinventar."}</p></div><Link className={ui.button} to="/enrollments/new">LXC aufnehmen</Link></header><div className={cn(ui.callout, ui.calloutInfo)}><strong>Was erscheint hier?</strong><p>Nur LXC-Container, die über einen Node entdeckt wurden. Ein Zugangsprofil allein erzeugt keinen Inventareintrag. Verbinde einen entdeckten LXC über „LXC aufnehmen“ mit seinem Zugang.</p></div><section className={ui.panel}>{containerBody(containers, visibleContainers)}</section></>;
}

function containerBody(containers: ReturnType<typeof useContainers>, visibleContainers: ContainerDto[]) {
  if (containers.isLoading) return <p className={ui.muted}>Container werden geladen…</p>;
  if (containers.error) return <p className={ui.errorState}>{containers.error.message}</p>;
  return <DataTable columns={columns} data={visibleContainers} emptyMessage="Keine LXC-Container entdeckt. Prüfe zuerst die Node-Inventarisierung." />;
}
