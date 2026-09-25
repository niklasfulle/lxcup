import { cn } from "../classnames";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createColumnHelper } from "@tanstack/react-table";
import { DataTable } from "../components/DataTable";
import { discoverDockerWorkloads } from "../api";
import { dockerDiscoveryFailureLabel } from "../dockerDiscoveryStatus";
import { queryKeys, useContainers, useDockerDiscovery } from "../queries";
import { Link, useSearchParams } from "react-router-dom";
import type { ContainerDto } from "../api";

const column = createColumnHelper<ContainerDto>();
const columns = [column.accessor("id", { header: "VMID" }), column.accessor("name", { header: "Name", cell: (info) => <Link to={`/containers/${info.row.original.id}`}>{info.getValue()}</Link> }), column.accessor("operating_system", { header: "OS" }), column.accessor("status", { header: "Status" }), column.accessor("management_state", { header: "Verwaltung" }), column.display({ id: "docker", header: "Docker-Inventar", cell: (info) => <ContainerDockerDiscovery container={info.row.original} /> })];

function ContainerDockerDiscovery({ container }: Readonly<{ container: ContainerDto }>) {
  const discovery = useDockerDiscovery(container.id);
  const queryClient = useQueryClient();
  const discover = useMutation({
    mutationFn: () => discoverDockerWorkloads(container.id),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerDiscovery(container.id) });
      void queryClient.invalidateQueries({ queryKey: queryKeys.dockerWorkloads(container.id) });
    },
  });
  const last = discovery.data;
  const label = discover.isPending || last?.status === "running" ? "Läuft" : last?.status === "succeeded" ? `${last.container_count} Container` : last?.status === "failed" ? dockerDiscoveryFailureLabel(last.error_code ?? "Docker-Erkennung fehlgeschlagen") : "Noch nicht geprüft";
  return <div className="flex flex-wrap items-center gap-2"><span className="text-xs text-[var(--muted)]">{discovery.isLoading ? "Lädt…" : label}</span><button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1 text-xs font-semibold text-[var(--ink)] hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={discover.isPending} onClick={() => discover.mutate()}>{discover.isPending ? "Erkennung läuft…" : "Erkennen"}</button><Link className="text-xs font-semibold text-lxcup-primary hover:underline" to={`/docker?host=${container.id}`}>Öffnen</Link>{discover.error ? <span className="basis-full text-xs text-[var(--error)]" role="alert">{dockerDiscoveryFailureLabel(discover.error.message)}</span> : null}</div>;
}

export function ContainersPage() {
  const containers = useContainers();
  const [searchParams] = useSearchParams();
  const nodeFilter = searchParams.get("node");
  const visibleContainers = nodeFilter ? (containers.data ?? []).filter((container) => container.node_id === nodeFilter) : (containers.data ?? []);
  return <><header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">LXC-Container</p><h1>LXC-Inventar</h1><p className="text-[var(--muted)]">{nodeFilter ? "Gefiltert nach ausgewähltem Node" : "Entdeckte LXC-Container aus dem Infrastrukturinventar."}</p></div><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to="/enrollments/new">LXC aufnehmen</Link></header><div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}><strong>Was erscheint hier?</strong><p>Nur LXC-Container, die über einen Node entdeckt wurden. Ein Zugangsprofil allein erzeugt keinen Inventareintrag. Verbinde einen entdeckten LXC über „LXC aufnehmen“ mit seinem Zugang.</p></div><section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">{containerBody(containers, visibleContainers)}</section></>;
}

function containerBody(containers: ReturnType<typeof useContainers>, visibleContainers: ContainerDto[]) {
  if (containers.isLoading) return <p className="text-[var(--muted)]">Container werden geladen…</p>;
  if (containers.error) return <p className="font-semibold text-[var(--error)]">{containers.error.message}</p>;
  return <DataTable columns={columns} data={visibleContainers} emptyMessage="Keine LXC-Container entdeckt. Prüfe zuerst die Node-Inventarisierung." />;
}
