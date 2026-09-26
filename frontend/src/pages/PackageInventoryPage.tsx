import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { cn } from "../classnames";
import { createAnsibleJob, type AnsibleJobDto } from "../api";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { queryKeys, useAnsibleJobs, usePackageInventory, useTargets } from "../queries";

function packageCandidateLabel(installed: string, candidate: string | null) {
  if (candidate === null) return "Noch nicht geprüft";
  if (candidate === installed) return "Aktuell";
  return `${candidate} · Update verfügbar`;
}

function targetListPath(kind: string | undefined) {
  if (kind === "lxc") return "/containers";
  if (kind === "windows_server") return "/windows";
  return "/servers";
}

function inventoryStatusLabel(status: string | undefined, loading: boolean) {
  if (status === "complete") return "Abgeschlossen";
  if (status === "not_collected") return "Noch nicht erhoben";
  return loading ? "Wird geladen…" : "Unbekannt";
}

function selectInventoryJob(jobs: NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>, targetId: string | undefined) {
  return jobs
    .filter((job) => job.operation === "collect_package_inventory" && "target" in job.target && job.target.target === targetId)
    .sort((left, right) => right.created_at.localeCompare(left.created_at));
}

function hasActiveInventoryJob(jobs: NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>, submittedJob: AnsibleJobDto | undefined) {
  const activeStatuses = (status: string) => ["queued", "checking", "planned", "applying"].includes(status);
  return jobs.some((job) => activeStatuses(job.status)) || Boolean(
    submittedJob && !jobs.some((job) => job.id === submittedJob.id) && activeStatuses(submittedJob.status),
  );
}

function sortPackages(packages: NonNullable<ReturnType<typeof usePackageInventory>["data"]>["packages"], query: string, sortBy: "name" | "version") {
  const matchesQuery = (name: string, installed: string, candidate: string | null) => !query || name.toLowerCase().includes(query) || installed.toLowerCase().includes(query) || (candidate?.toLowerCase().includes(query) ?? false);
  const compare = sortBy === "name"
    ? (left: typeof packages[number], right: typeof packages[number]) => left.name.localeCompare(right.name)
    : (left: typeof packages[number], right: typeof packages[number]) => left.installed_version.localeCompare(right.installed_version);
  return [...packages].filter((item) => matchesQuery(item.name, item.installed_version, item.candidate_version)).sort(compare);
}

export function PackageInventoryPage() {
  const { targetId } = useParams();
  const queryClient = useQueryClient();
  const inventory = usePackageInventory(targetId);
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const [submittedInventoryJob, setSubmittedInventoryJob] = useState<AnsibleJobDto>();
  const refreshedInventoryJob = useRef<string | undefined>(undefined);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim().toLowerCase());
  const [sortBy, setSortBy] = useState<"name" | "version">("name");
  const target = targets.data?.find((item) => item.id === targetId);
  const inventoryJobs = selectInventoryJob(jobs.data ?? [], targetId);
  const inventoryJob = submittedInventoryJob
    ? inventoryJobs.find((job) => job.id === submittedInventoryJob.id) ?? submittedInventoryJob
    : inventoryJobs[0];
  const hasActiveJob = hasActiveInventoryJob(inventoryJobs, submittedInventoryJob);
  const collectInventory = useMutation({
    mutationFn: () => createAnsibleJob({
      operation: "collect_package_inventory",
      target_id: targetId,
      mode: "check",
      parameters: { operation: "collect_package_inventory" },
      idempotency_key: `inventory-refresh-${targetId}-${crypto.randomUUID()}`,
      confirmed: true,
    }),
    onSuccess: (job) => {
      setSubmittedInventoryJob(job);
      queryClient.setQueryData<AnsibleJobDto[]>(queryKeys.ansibleJobs, (current) => [job, ...(current ?? []).filter((item) => item.id !== job.id)]);
      void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs });
    },
  });

  useEffect(() => {
    if (!targetId || inventoryJob?.status !== "succeeded" || refreshedInventoryJob.current === inventoryJob.id) return;
    refreshedInventoryJob.current = inventoryJob.id;
    void queryClient.invalidateQueries({ queryKey: queryKeys.packageInventory(targetId) });
  }, [inventoryJob, queryClient, targetId]);
  const packages = useMemo(() => {
    const all = inventory.data?.packages ?? [];
    return sortPackages(all, deferredQuery, sortBy);
  }, [deferredQuery, inventory.data?.packages, sortBy]);

  const collectionButtonLabel = inventoryCollectionButtonLabel(collectInventory.isPending, hasActiveJob);

  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Systeminventar</p><h1>Paketinventar</h1><p className="text-[var(--muted)]">{target ? `${target.name} · ${target.kind}` : "Installierte Pakete des verwalteten Systems"}</p></div><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to={targetListPath(target?.kind)}>← Zur Ressourcenübersicht</Link></header>
    <InventorySection inventory={inventory} inventoryJob={inventoryJob} target={target} hasActiveJob={hasActiveJob} collectionPending={collectInventory.isPending} collectionButtonLabel={collectionButtonLabel} onCollect={() => collectInventory.mutate()} collectionError={collectInventory.error} query={query} onQueryChange={setQuery} sortBy={sortBy} onSortChange={setSortBy} packages={packages} />
  </>;
}

function inventoryCollectionButtonLabel(pending: boolean, active: boolean) {
  if (pending) return "Wird eingereiht…";
  if (active) return "Inventarisierung läuft";
  return "Inventarisierung starten";
}

function InventorySection({ inventory, inventoryJob, target, hasActiveJob, collectionPending, collectionButtonLabel, onCollect, collectionError, query, onQueryChange, sortBy, onSortChange, packages }: Readonly<{
  inventory: ReturnType<typeof usePackageInventory>;
  inventoryJob: AnsibleJobDto | undefined;
  target: NonNullable<ReturnType<typeof useTargets>["data"]>[number] | undefined;
  hasActiveJob: boolean;
  collectionPending: boolean;
  collectionButtonLabel: string;
  onCollect: () => void;
  collectionError: unknown;
  query: string;
  onQueryChange: (value: string) => void;
  sortBy: "name" | "version";
  onSortChange: (value: "name" | "version") => void;
  packages: NonNullable<ReturnType<typeof usePackageInventory>["data"]>["packages"];
}>) {
  const collectionDisabled = collectionPending || hasActiveJob || target?.state !== "managed";
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]">
    <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
      <div className="grid gap-1"><span className="text-[var(--muted)]">Erhebungsstatus: {inventoryStatusLabel(inventory.data?.status, inventory.isLoading)}</span>
        {inventoryJob ? <Link className={cn("inline-flex w-fit items-center border border-transparent px-2 py-1 text-xs font-semibold hover:border-[var(--primary)] hover:underline", jobStatusBadgeClass(inventoryJob.status))} to={`/workflows/${inventoryJob.id}`}>Inventarisierungs-Workflow · {jobStatusLabel(inventoryJob.status)}</Link> : <span className="text-xs text-[var(--muted)]">Kein Inventarisierungs-Workflow vorhanden</span>}
      </div>
      {target ? <button className="inline-flex min-h-9 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={collectionDisabled} onClick={onCollect}>{collectionButtonLabel}</button> : null}
    </div>
    {target && target.state !== "managed" ? <p className="text-xs text-[var(--muted)]">Die Erfassung ist erst möglich, wenn das Ziel verbunden ist.</p> : null}
    {collectionError instanceof Error ? <p className="font-semibold text-[var(--error)]" role="alert">Inventarisierung konnte nicht gestartet werden: {collectionError.message}</p> : null}
    {inventory.isLoading ? <p className="text-[var(--muted)]">Paketinventar wird geladen…</p> : null}
    {inventory.error ? <p className="font-semibold text-[var(--error)]" role="alert">{inventory.error.message}</p> : null}
    {inventory.data?.status === "not_collected" ? <div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}><strong>Noch kein Paketinventar</strong><p>Die erste Inventarisierung wird nach dem Onboarding eingereiht. Anschließend erscheint hier die vollständige, durchsuchbare Paketliste.</p></div> : null}
    {inventory.data?.status === "complete" ? <PackageTable inventory={inventory.data} packages={packages} query={query} onQueryChange={onQueryChange} sortBy={sortBy} onSortChange={onSortChange} /> : null}
  </section>;
}

function PackageTable({ inventory, packages, query, onQueryChange, sortBy, onSortChange }: Readonly<{
  inventory: NonNullable<ReturnType<typeof usePackageInventory>["data"]>;
  packages: NonNullable<ReturnType<typeof usePackageInventory>["data"]>["packages"];
  query: string;
  onQueryChange: (value: string) => void;
  sortBy: "name" | "version";
  onSortChange: (value: "name" | "version") => void;
}>) {
  const collectedAt = inventory.collected_at ? new Date(inventory.collected_at).toLocaleString() : "unbekannt";
  return <>
    <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>{inventory.packages.length} installierte Pakete</h2><p className="text-[var(--muted)]">Zuletzt erhoben: {collectedAt}</p></div></div>
    <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3"><label><span>Suche nach Paket oder Version</span><input value={query} onChange={(event) => onQueryChange(event.target.value)} placeholder="z. B. curl oder 8.5" /></label><label><span>Sortierung</span><select value={sortBy} onChange={(event) => onSortChange(event.target.value as "name" | "version")}><option value="name">Paketname</option><option value="version">Installierte Version</option></select></label></div>
    {packages.length === 0 ? <p className="m-0 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Keine Pakete entsprechen dem aktuellen Filter.</p> : <div className="overflow-x-auto"><table><thead><tr><th>Paket</th><th>Installiert</th><th>Neueste Paketquellen-Version</th><th>Architektur</th><th>Quelle</th></tr></thead><tbody>{packages.map((item) => <tr key={`${item.name}-${item.installed_version}-${item.architecture ?? ""}`}><td><strong>{item.name}</strong></td><td>{item.installed_version}</td><td><span className={item.candidate_version && item.candidate_version !== item.installed_version ? "font-semibold text-[var(--warning)]" : "text-[var(--muted)]"}>{packageCandidateLabel(item.installed_version, item.candidate_version)}</span></td><td>{item.architecture ?? "—"}</td><td>{item.source ?? "—"}</td></tr>)}</tbody></table></div>}
  </>;
}
