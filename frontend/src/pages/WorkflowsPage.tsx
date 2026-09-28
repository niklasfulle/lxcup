import { cn } from "../classnames";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { useMemo, useState } from "react";
import { useMutation, useQueries, useQueryClient, type QueryFunctionContext } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { createAnsibleJob, getPackageInventory, type AnsibleExecutionMode, type AnsibleJobDto, type AnsibleOperation, type CreateAnsibleJobRequest, type PackageInventoryDto, type TargetDto } from "../api";
import { queryKeys, useAnsibleJobEvents, useAnsibleJobs, useTargets, useUpdatePolicies } from "../queries";
import workflowModeMatrix from "../../workflow-modes.json";

const operationModes = workflowModeMatrix as Record<AnsibleOperation, AnsibleExecutionMode[]>;
type WorkflowSubmissionResult = Readonly<{ target: TargetDto; job: AnsibleJobDto | null; error: string | null }>;

const operations: Array<{ value: AnsibleOperation; label: string; risk: string }> = [
  { value: "deploy_agent", label: "Agent installieren", risk: "Ändernd" },
  { value: "update_agent", label: "Agent aktualisieren", risk: "Ändernd" },
  { value: "repair_agent", label: "Agent reparieren", risk: "Ändernd" },
  { value: "update_packages", label: "Pakete aktualisieren", risk: "Ändernd" },
  { value: "health_check", label: "Healthcheck", risk: "Nur lesend" },
  { value: "collect_package_inventory", label: "Paketinventar erfassen", risk: "Nur lesend" },
];

const modes: Array<{ value: AnsibleExecutionMode; label: string; effect: string; description: string }> = [
  { value: "check", label: "Check", effect: "Prüfung", description: "Prüft Voraussetzungen und den aktuellen Zustand für die gewählte Operation. Der Job dokumentiert das Ergebnis, ohne eine freigegebene Änderung auszuführen." },
  { value: "plan", label: "Plan / Dry-Run", effect: "Vorschau", description: "Erstellt eine Vorschau der vorgesehenen Schritte. Nutze diesen Modus, um Umfang und erwartete Änderungen vor dem Apply zu prüfen." },
  { value: "apply", label: "Apply", effect: "Ausführung", description: "Führt die gewählte Operation auf dem Ziel aus. Bei ändernden Operationen ist deshalb die ausdrückliche Bestätigung erforderlich." },
];

export function buildWorkflowRequest(
  targetId: string,
  operation: AnsibleOperation,
  mode: AnsibleExecutionMode,
  packages: readonly string[] | string,
  confirmed: boolean,
  policyId?: string,
  approvedPlanJobId?: string,
  latestAgentVersion?: string,
): CreateAnsibleJobRequest {
  let parameters: Record<string, unknown> = { operation };
  if (operation === "update_packages") {
    const selectedPackages = typeof packages === "string"
      ? packages.split(",").map((item) => item.trim()).filter(Boolean)
      : [...packages];
    parameters = { operation, packages: selectedPackages.length > 0 ? selectedPackages : ["*"] };
  } else if (operation === "deploy_agent" || operation === "update_agent" || operation === "repair_agent") {
    if (!latestAgentVersion) throw new Error("Die aktuelle Agent-Version konnte nicht vom Controller geladen werden.");
    parameters = { operation, agent_version: latestAgentVersion };
  }

  return {
    operation,
    target_id: targetId,
    mode,
    parameters,
    idempotency_key: workflowIdempotencyKey(operation, mode, policyId, approvedPlanJobId),
    confirmed,
    ...(operation === "update_packages" ? { policy_id: policyId, approved_plan_job_id: approvedPlanJobId } : {}),
  };
}

function workflowIdempotencyKey(operation: AnsibleOperation, mode: AnsibleExecutionMode, policyId?: string, approvedPlanJobId?: string) {
  if (operation === "update_packages" && mode === "plan" && policyId) return `package-plan:${policyId}:${crypto.randomUUID()}`;
  if (operation === "update_packages" && mode === "apply" && approvedPlanJobId) return `package-apply:${approvedPlanJobId}:${crypto.randomUUID()}`;
  return crypto.randomUUID();
}

export function WorkflowsPage() {
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const policies = useUpdatePolicies();
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const targetFilter = searchParams.get("target");
  const [targetIds, setTargetIds] = useState<string[]>(() => targetFilter ? [targetFilter] : []);
  const [bulkMode, setBulkMode] = useState(false);
  const [operation, setOperation] = useState<AnsibleOperation>("health_check");
  const [mode, setMode] = useState<AnsibleExecutionMode>("check");
  const [packagesByTarget, setPackagesByTarget] = useState<Record<string, string[]>>({});
  const [confirmed, setConfirmed] = useState(false);
  const [policyId, setPolicyId] = useState("");
  const [approvedPlanJobIds, setApprovedPlanJobIds] = useState<Record<string, string>>({});
  const packageInventories = useQueries({ queries: targetIds.map((targetId) => ({
    queryKey: queryKeys.packageInventory(targetId),
    queryFn: ({ signal }: QueryFunctionContext) => getPackageInventory(targetId, signal),
    enabled: operation === "update_packages",
    staleTime: 10_000,
  })) });
  const mutation = useMutation({
    mutationFn: async ({ selectedTargets, planIds }: { selectedTargets: TargetDto[]; planIds: Record<string, string> }) => {
      const outcomes = await Promise.allSettled(selectedTargets.map((target) => createAnsibleJob(buildWorkflowRequest(target.id, operation, mode, packagesByTarget[target.id] ?? [], confirmed, policyId || undefined, planIds[target.id] || undefined, target.latest_agent_version))));
      return outcomes.map((outcome, index): WorkflowSubmissionResult => outcome.status === "fulfilled"
        ? { target: selectedTargets[index], job: outcome.value, error: null }
        : { target: selectedTargets[index], job: null, error: outcome.reason instanceof Error ? outcome.reason.message : "Unbekannter Fehler" });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs });
    },
  });
  const selectedOperation = useMemo(() => operations.find((item) => item.value === operation), [operation]);
  const selectedMode = useMemo(() => modes.find((item) => item.value === mode), [mode]);
  const selectedTargets = (targets.data ?? []).filter((target) => targetIds.includes(target.id));
  const pendingTargets = selectedTargets.filter((target) => target.state === "pending");
  const visibleJobs = jobs.data?.filter((job) => targetFilter === null || ("target" in job.target && job.target.target === targetFilter));
  const isModifyingOperation = operation !== "health_check" && operation !== "collect_package_inventory";
  const packagePlansByTarget = Object.fromEntries(targetIds.map((targetId) => {
    const selectedPackages = packagesByTarget[targetId] ?? [];
    const requestedPackageScope = (selectedPackages.length > 0 ? selectedPackages : ["*"]).slice().sort(compareText).join(",");
    return [targetId, (jobs.data ?? []).filter((job) => job.operation === "update_packages" && job.mode === "plan" && job.status === "succeeded" && "target" in job.target && job.target.target === targetId && job.update_policy_id === policyId && [...(job.package_names ?? [])].sort(compareText).join(",") === requestedPackageScope)];
  }));
  const selectedPolicy = policies.data?.find((policy) => policy.id === policyId && policy.enabled);
  const canSubmit = canSubmitWorkflow({ targetIds, allTargetsExist: selectedTargets.length === targetIds.length, isModifyingOperation, confirmed, operation, policyId, policyTargetIds: selectedPolicy?.allowed_targets ?? [], mode, approvedPlanJobIds, packagePlansByTarget, supportedModes: operationModes[operation] });

  function submit(event: Readonly<{ preventDefault: () => void }>) {
    event.preventDefault();
    if (canSubmit === false) return;
    mutation.mutate({ selectedTargets, planIds: approvedPlanJobIds });
  }

  return <>
      <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Ansible Worker</p><h1>Automatisierung</h1><p className="text-[var(--muted)]">Freigegebene Workflows für Agenten und Paketpläne.</p></div></header>
      <WorkflowControlPanel
        targets={targets.data ?? []} targetIds={targetIds} onTargetToggle={(targetId, selected) => setTargetIds((current) => selected ? [...new Set([...current, targetId])] : current.filter((id) => id !== targetId))}
        bulkMode={bulkMode} onBulkModeChange={(enabled) => { setBulkMode(enabled); if (!enabled) setTargetIds((current) => current.slice(0, 1)); }}
        onSingleTargetChange={(targetId) => setTargetIds(targetId ? [targetId] : [])}
        onSelectAllTargets={() => setTargetIds((targets.data ?? []).map((target) => target.id))} onClearTargets={() => setTargetIds([])}
        operation={operation} onOperationChange={(next) => { setOperation(next); setMode(operationModes[next][0] ?? "check"); }}
        mode={mode} onModeChange={setMode} packagesByTarget={packagesByTarget} onPackagesChange={setPackagesByTarget}
        packageInventories={Object.fromEntries(targetIds.map((targetId, index) => [targetId, packageInventories[index]?.data]))}
        policies={policies.data ?? []} policyId={policyId} onPolicyChange={setPolicyId}
        approvedPlanJobIds={approvedPlanJobIds} onApprovedPlanChange={(targetId, planId) => setApprovedPlanJobIds((current) => ({ ...current, [targetId]: planId }))}
        packagePlansByTarget={packagePlansByTarget} modifying={isModifyingOperation} confirmed={confirmed}
        onConfirmedChange={setConfirmed} pendingTargets={pendingTargets} selectedOperation={selectedOperation}
        selectedMode={selectedMode} pending={mutation.isPending} error={mutation.error}
        results={mutation.data} onSubmit={submit} canSubmit={canSubmit}
      />
      <section className="mb-3 overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)] shadow-sm">
        <div className="flex flex-wrap items-start justify-between gap-3 border-b border-[var(--line)] px-4 py-3">
          <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Ausführungsverlauf</p><h2 className="m-0">Workflow-Protokolle</h2><p className="mt-1 text-sm text-[var(--muted)]">{workflowHistoryDescription(targetFilter, targets.data ?? [])}</p></div>
          <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 py-1 text-xs font-semibold text-[var(--muted)]">{visibleJobs?.length ?? 0} Jobs</span>
        </div>
        {workflowSummary(visibleJobs)}
        {workflowJobsContent(jobs, visibleJobs, targets.data ?? [])}
      </section>
    </>;
}

type WorkflowControlPanelProps = Readonly<{
  targets: NonNullable<ReturnType<typeof useTargets>["data"]>;
  targetIds: string[];
  onTargetToggle: (targetId: string, selected: boolean) => void;
  bulkMode: boolean;
  onBulkModeChange: (enabled: boolean) => void;
  onSingleTargetChange: (targetId: string) => void;
  onSelectAllTargets: () => void;
  onClearTargets: () => void;
  operation: AnsibleOperation;
  onOperationChange: (value: AnsibleOperation) => void;
  mode: AnsibleExecutionMode;
  onModeChange: (value: AnsibleExecutionMode) => void;
  packagesByTarget: Record<string, string[]>;
  onPackagesChange: (value: Record<string, string[]>) => void;
  packageInventories: Record<string, PackageInventoryDto | undefined>;
  policies: NonNullable<ReturnType<typeof useUpdatePolicies>["data"]>;
  policyId: string;
  onPolicyChange: (value: string) => void;
  approvedPlanJobIds: Record<string, string>;
  onApprovedPlanChange: (targetId: string, planId: string) => void;
  packagePlansByTarget: Record<string, NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>>;
  modifying: boolean;
  confirmed: boolean;
  onConfirmedChange: (value: boolean) => void;
  pendingTargets: TargetDto[];
  selectedOperation: typeof operations[number] | undefined;
  selectedMode: typeof modes[number] | undefined;
  pending: boolean;
  error: unknown;
  results: WorkflowSubmissionResult[] | undefined;
  canSubmit: boolean;
  onSubmit: (event: Readonly<{ preventDefault: () => void }>) => void;
}>;

function WorkflowTypeToggle({ bulkMode, onChange }: Readonly<{ bulkMode: boolean; onChange: (enabled: boolean) => void }>) {
  const selectedClass = "bg-[var(--primary)] text-white";
  const idleClass = "text-[var(--muted)] hover:text-[var(--ink)]";
  return <div className="inline-flex border border-[var(--line)] bg-[var(--panel)] p-1" role="tablist" aria-label="Workflow-Typ">
    <button id="single-workflow-tab" className={cn("px-3 py-1.5 text-xs font-semibold transition-colors", bulkMode ? idleClass : selectedClass)} type="button" role="tab" aria-selected={bulkMode === false} aria-controls="single-workflow-panel" onClick={() => onChange(false)}>Einzelner Workflow</button>
    <button id="bulk-workflow-tab" className={cn("px-3 py-1.5 text-xs font-semibold transition-colors", bulkMode ? selectedClass : idleClass)} type="button" role="tab" aria-selected={bulkMode} aria-controls="bulk-workflow-panel" onClick={() => onChange(true)}>Bulk-Workflows</button>
  </div>;
}

function WorkflowTargetSelection({ bulkMode, targets, targetIds, onTargetToggle, onSingleTargetChange, onSelectAllTargets, onClearTargets }: Readonly<Pick<WorkflowControlPanelProps, "bulkMode" | "targets" | "targetIds" | "onTargetToggle" | "onSingleTargetChange" | "onSelectAllTargets" | "onClearTargets">>) {
  if (bulkMode) return <BulkTargetSelection targets={targets} targetIds={targetIds} onTargetToggle={onTargetToggle} onSelectAllTargets={onSelectAllTargets} onClearTargets={onClearTargets} />;
  return <div id="single-workflow-panel" role="tabpanel" aria-labelledby="single-workflow-tab" className="min-w-0">
    <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Das registrierte Ziel, gegen das der Workflow ausgeführt wird."><span>Ziel</span><select value={targetIds[0] ?? ""} onChange={(event) => onSingleTargetChange(event.target.value)}><option value="">Ziel auswählen</option>{targets.map((target) => <option key={target.id} value={target.id}>{target.name} · {target.kind} · {target.address}</option>)}</select></label>
  </div>;
}

function BulkTargetSelection({ targets, targetIds, onTargetToggle, onSelectAllTargets, onClearTargets }: Readonly<Pick<WorkflowControlPanelProps, "targets" | "targetIds" | "onTargetToggle" | "onSelectAllTargets" | "onClearTargets">>) {
  const countLabel = targetIds.length === 1 ? "Ziel ausgewählt" : "Ziele ausgewählt";
  return <div id="bulk-workflow-panel" role="tabpanel" aria-labelledby="bulk-workflow-tab" className="grid min-w-0 gap-3 md:col-span-2 xl:col-span-3">
    <div className="flex flex-wrap items-center justify-between gap-2"><p className="m-0 text-xs font-semibold text-[var(--muted)]">{targetIds.length} {countLabel}</p><div className="flex gap-2"><button className="border border-[var(--line)] bg-[var(--panel)] px-2.5 py-1.5 text-xs font-semibold transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)]" type="button" onClick={onSelectAllTargets} disabled={targets.length === 0}>Alle auswählen</button><button className="border border-[var(--line)] bg-[var(--panel)] px-2.5 py-1.5 text-xs font-semibold transition-colors hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onClearTargets} disabled={targetIds.length === 0}>Auswahl leeren</button></div></div>
    {targets.length === 0 ? <p className="m-0 border border-dashed border-[var(--line)] px-3 py-4 text-sm text-[var(--muted)]">Keine Ziele verfügbar.</p> : <div className="grid max-h-56 grid-cols-1 gap-2 overflow-y-auto sm:grid-cols-[repeat(auto-fill,minmax(min(100%,18rem),26rem))]">{targets.map((target) => <WorkflowTargetOption key={target.id} target={target} selected={targetIds.includes(target.id)} onToggle={onTargetToggle} />)}</div>}
  </div>;
}

function WorkflowTargetOption({ target, selected, onToggle }: Readonly<{ target: TargetDto; selected: boolean; onToggle: (targetId: string, selected: boolean) => void }>) {
  const className = selected ? "border-[var(--primary)] bg-[var(--primary-soft)]" : "border-[var(--line)] bg-[var(--panel)] hover:border-[var(--primary)]/60 hover:bg-[var(--paper)]";
  const iconClass = selected ? "border-[var(--primary)]/40 bg-[var(--panel)] text-lxcup-primary" : "border-[var(--line)] bg-[var(--paper-muted)] text-[var(--muted)]";
  return <label className={cn("group flex min-h-[4.25rem] min-w-0 cursor-pointer items-center gap-3 border px-3 py-2.5 text-xs transition-colors focus-within:outline focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-[var(--primary)]", className)}>
    <input className="h-4 w-4 shrink-0 accent-lxcup-primary" type="checkbox" aria-label={`Ziel auswählen: ${target.name}`} checked={selected} onChange={(event) => onToggle(target.id, event.target.checked)} />
    <span className={cn("grid h-9 w-9 shrink-0 place-items-center border text-[10px] font-bold uppercase tracking-wide", iconClass)}>{workflowTargetKindShortLabel(target.kind)}</span>
    <span className="grid min-w-0 flex-1 gap-0.5" title={`${target.name} · ${target.kind} · ${target.address}`}><strong className="truncate text-sm text-[var(--ink)]">{target.name}</strong><span className="truncate text-[var(--muted)]">{workflowTargetKindLabel(target.kind)} <span aria-hidden="true">·</span> {target.address}</span></span>
  </label>;
}

function workflowSubmitLabel(targetCount: number, pending: boolean) {
  if (pending) return "Wird gestartet…";
  if (targetCount > 1) return `${targetCount} Workflows starten`;
  return "Workflow starten";
}

function WorkflowControlPanel(props: WorkflowControlPanelProps) {
  const { targets, targetIds, onTargetToggle, bulkMode, onBulkModeChange, onSingleTargetChange, onSelectAllTargets, onClearTargets, operation, onOperationChange, mode, onModeChange, packagesByTarget, onPackagesChange, packageInventories, policies, policyId, onPolicyChange, approvedPlanJobIds, onApprovedPlanChange, packagePlansByTarget, modifying, confirmed, onConfirmedChange, pendingTargets, selectedOperation, selectedMode, pending, error, results, canSubmit, onSubmit } = props;
  const selectedPolicy = policies.find((policy) => policy.id === policyId && policy.enabled);
  const policyCoversTargets = selectedPolicy !== undefined && targetIds.every((targetId) => selectedPolicy.allowed_targets.includes(targetId));
  const policyWarning = operation === "update_packages" && policyId && targetIds.length > 0 && !policyCoversTargets;
  const pendingTargetLabel = pendingTargets.length === 1 ? "Ziel wartet noch auf seinen Agenten" : `${pendingTargets.length} Ziele warten noch auf ihre Agenten`;
  const submitLabel = workflowSubmitLabel(targetIds.length, pending);
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)] shadow-sm">
        <form className="space-y-4" onSubmit={onSubmit}>
          <div className="flex flex-wrap items-start justify-between gap-3 border-b border-[var(--line)] pb-3">
            <div>
              <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Workflow-Steuerung</p>
              <h2 className="m-0">Workflow starten</h2>
              <p className="mt-1 text-[var(--muted)]">Wähle ein oder mehrere Ziele, eine freigegebene Aktion und den Ausführungsmodus.</p>
            </div>
            <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 py-1 text-xs font-semibold text-[var(--muted)]">Registry-gesichert</span>
          </div>
          <div className="grid grid-cols-1 gap-4 rounded border border-[var(--line)] bg-[var(--paper-muted)] p-4 md:grid-cols-2 xl:grid-cols-3">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3 md:col-span-2 xl:col-span-3">
              <div><h3 className="m-0 text-xs font-bold uppercase tracking-wider text-[var(--muted)]">Workflow-Typ</h3><p className="mb-0 mt-1 text-xs text-[var(--muted)]">Einzelne Ziele wie bisher oder dieselbe Aufgabe für mehrere Ziele einreihen.</p></div>
              <WorkflowTypeToggle bulkMode={bulkMode} onChange={onBulkModeChange} />
            </div>
            <WorkflowTargetSelection bulkMode={bulkMode} targets={targets} targetIds={targetIds} onTargetToggle={onTargetToggle} onSingleTargetChange={onSingleTargetChange} onSelectAllTargets={onSelectAllTargets} onClearTargets={onClearTargets} />
            <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Die erlaubte, fest registrierte Aktion des Workers."><span>Operation</span>
                <select value={operation} onChange={(event) => onOperationChange(event.target.value as AnsibleOperation)}>
                {operations.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
            <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Check prüft, Plan erstellt eine Vorschau, Apply führt aus und Reconcile gleicht einen unklaren Zustand ab."><span>Modus</span>
              <select value={mode} onChange={(event) => onModeChange(event.target.value as AnsibleExecutionMode)} aria-describedby="mode-help">
              {modes.filter((item) => operationModes[operation].includes(item.value)).map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
          </div>
          {operation === "update_packages" ? <PackageUpdatePicker targets={targets} targetIds={targetIds} packagesByTarget={packagesByTarget} onPackagesChange={onPackagesChange} inventories={packageInventories} /> : null}
          {operation === "update_packages" ? <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
            <label><span>Freigegebene Update-Policy</span><select value={policyId} onChange={(event) => onPolicyChange(event.target.value)}><option value="">Policy auswählen</option>{policies.filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id} · max. Risiko {policy.maximum_risk}</option>)}</select></label>
            {mode === "apply" ? targetIds.map((targetId) => <label key={targetId} className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]"><span>Plan für {targets.find((target) => target.id === targetId)?.name ?? targetId}</span><select value={approvedPlanJobIds[targetId] ?? ""} onChange={(event) => onApprovedPlanChange(targetId, event.target.value)}><option value="">Erfolgreichen Plan auswählen</option>{(packagePlansByTarget[targetId] ?? []).map((job) => <option key={job.id} value={job.id}>{job.id} · {new Date(job.updated_at).toLocaleString()}</option>)}</select></label>) : null}
            {mode === "plan" ? <p className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}>Nach erfolgreichem Plan kannst du denselben Ziel- und Paketumfang mit Apply ausdrücklich bestätigen.</p> : null}
          </div> : null}
          {policyWarning ? <output className="block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Die gewählte Policy erlaubt nicht alle ausgewählten Ziele. Wähle eine Policy, die jedes Ziel freigibt.</output> : null}
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(16rem,0.8fr)]">
            {selectedMode ? <div id="mode-help" className="border border-[var(--line)] bg-[var(--primary-soft)] p-3 text-[var(--ink)]" role="note"><div className="mb-1 flex flex-wrap items-center gap-2"><strong>{selectedMode.label}: {selectedMode.effect}</strong><span className="border border-[var(--line)] bg-[var(--panel)] px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-[var(--muted)]">{selectedOperation?.risk}</span></div><p className="m-0 text-sm text-[var(--muted)]">{selectedMode.description}</p></div> : null}
            <div className="flex flex-col justify-center gap-1 border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-2 text-sm"><strong className="text-xs uppercase tracking-wide text-[var(--muted)]">Sicherheitsrahmen</strong><span>Playbook und Secret-Auflösung kommen ausschließlich aus der Registry.</span></div>
          </div>
          {pendingTargets.length > 0 ? <output className="block border border-[#bad0fa] bg-[var(--primary-soft)] p-3 text-sm text-[var(--ink)]"><strong>{pendingTargetLabel}</strong><span className="mt-1 block text-[var(--muted)]">{pendingTargets.map((target) => target.name).join(", ")}. Mit „Agent installieren“ kannst du das Onboarding starten.</span></output> : null}
          <div className="flex flex-col gap-3 border-t border-[var(--line)] pt-4 sm:flex-row sm:items-center sm:justify-between">
            {modifying ? <label className="flex items-center gap-2 text-sm font-medium"><input className="h-4 w-4 accent-lxcup-primary" type="checkbox" checked={confirmed} onChange={(event) => onConfirmedChange(event.target.checked)} /> {workflowConfirmationLabel(mode, targetIds.length)}</label> : <span className="text-xs text-[var(--muted)]">Für jedes Ziel wird ein eigener Job eingereiht.</span>}
            <button className="inline-flex min-h-10 shrink-0 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-5 py-2 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={!canSubmit || pending}>{submitLabel}<span aria-hidden="true">→</span></button>
          </div>
          {error instanceof Error ? <p className="m-0 border border-[var(--error)] bg-[var(--error-soft)] px-3 py-2 font-semibold text-[var(--error)]" role="alert">Workflow konnte nicht gestartet werden: {error.message}</p> : null}
          {results?.length ? <output className="grid gap-2 border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-sm"><strong>{results.filter((result) => result.job).length} von {results.length} Workflows eingereiht</strong><ul className="m-0 grid list-none gap-1 p-0">{results.map((result) => <li key={result.target.id} className="flex flex-wrap items-center justify-between gap-2 border-t border-[var(--line)] pt-2"><span className="font-medium">{result.target.name}</span>{result.job ? <span className="flex items-center gap-2"><span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", jobStatusBadgeClass(result.job.status))}>{jobStatusLabel(result.job.status)}</span><Link className="text-xs font-semibold text-lxcup-primary hover:underline" to={`/workflows/${result.job.id}`}>Protokoll öffnen →</Link></span> : <span className="text-xs font-semibold text-[var(--error)]" role="alert">Nicht eingereiht: {result.error}</span>}</li>)}</ul></output> : null}
        </form>
      </section>;
}

function PackageUpdatePicker({ targets, targetIds, packagesByTarget, onPackagesChange, inventories }: Readonly<{
  targets: TargetDto[];
  targetIds: string[];
  packagesByTarget: Record<string, string[]>;
  onPackagesChange: (value: Record<string, string[]>) => void;
  inventories: Record<string, PackageInventoryDto | undefined>;
}>) {
  const [open, setOpen] = useState(false);
  const [activeTargetId, setActiveTargetId] = useState("");
  const targetId = targetIds.includes(activeTargetId) ? activeTargetId : targetIds[0] ?? "";
  const target = targets.find((item) => item.id === targetId);
  const inventory = inventories[targetId];
  const availableUpdates = (inventory?.packages ?? []).filter((item) => item.candidate_version !== null && item.candidate_version !== item.installed_version);
  const selectedPackages = packagesByTarget[targetId] ?? [];

  function setPackageSelected(packageName: string, selected: boolean) {
    const nextPackages = selected
      ? [...new Set([...selectedPackages, packageName])]
      : selectedPackages.filter((name) => name !== packageName);
    onPackagesChange({ ...packagesByTarget, [targetId]: nextPackages });
  }

  const packageSummary = targetIds.length === 0 ? "Ziel auswählen" : targetIds.map((id) => packageSelectionLabel(targets, packagesByTarget, id)).join(" · ");
  return <div className="grid gap-2 md:col-span-2 xl:col-span-3">
    <div className="flex flex-wrap items-center gap-3">
      <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={!targetId} onClick={() => setOpen(true)}>Updates auswählen</button>
      <span className="text-xs text-[var(--muted)]">{targetIds.length > 1 ? "Paketumfang je Ziel" : "Paketumfang"}: {packageSummary}</span>
    </div>
    {open ? <PackageUpdateDialog targetId={targetId} targetName={target?.name} targetIds={targetIds} targets={targets} inventory={inventory} availableUpdates={availableUpdates} selectedPackages={selectedPackages} onTargetChange={setActiveTargetId} onPackageSelected={setPackageSelected} onClearSelection={() => onPackagesChange({ ...packagesByTarget, [targetId]: [] })} onClose={() => setOpen(false)} /> : null}
  </div>;
}

function packageSelectionLabel(targets: TargetDto[], packagesByTarget: Record<string, string[]>, targetId: string) {
  const count = packagesByTarget[targetId]?.length ?? 0;
  const name = targets.find((item) => item.id === targetId)?.name ?? targetId;
  const scope = count === 0 ? "alle Updates" : `${count} ausgewählt`;
  return `${name}: ${scope}`;
}

function PackageUpdateDialog({ targetId, targetName, targetIds, targets, inventory, availableUpdates, selectedPackages, onTargetChange, onPackageSelected, onClearSelection, onClose }: Readonly<{
  targetId: string;
  targetName: string | undefined;
  targetIds: string[];
  targets: TargetDto[];
  inventory: PackageInventoryDto | undefined;
  availableUpdates: PackageInventoryDto["packages"];
  selectedPackages: string[];
  onTargetChange: (targetId: string) => void;
  onPackageSelected: (packageName: string, selected: boolean) => void;
  onClearSelection: () => void;
  onClose: () => void;
}>) {
  return <dialog open className="fixed inset-0 z-50 m-0 grid h-full w-full max-h-none max-w-none place-items-center overflow-hidden bg-black/60 p-4" aria-modal="true" aria-labelledby="package-picker-title">
    <section className="grid max-h-[min(85vh,48rem)] w-full max-w-3xl grid-rows-[auto_auto_1fr_auto] overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)] shadow-xl">
      <header className="flex flex-wrap items-start justify-between gap-3 border-b border-[var(--line)] p-4"><div><h2 id="package-picker-title" className="m-0">Verfügbare Updates</h2><p className="mb-0 mt-1 text-sm text-[var(--muted)]">Wähle einzelne Pakete aus. Bleibt die Auswahl leer, werden alle verfügbaren Updates eingeplant.</p></div><button className="border border-[var(--line)] px-3 py-1.5 text-xs font-semibold hover:bg-[var(--paper-muted)]" type="button" onClick={onClose}>Schließen</button></header>
      <PackageUpdateDialogTarget targetId={targetId} targetName={targetName} targetIds={targetIds} targets={targets} inventory={inventory} availableCount={availableUpdates.length} onTargetChange={onTargetChange} />
      <PackageUpdateDialogList targetId={targetId} inventory={inventory} availableUpdates={availableUpdates} selectedPackages={selectedPackages} onPackageSelected={onPackageSelected} />
      <PackageUpdateDialogFooter selectedCount={selectedPackages.length} onClearSelection={onClearSelection} onClose={onClose} />
    </section>
  </dialog>;
}

function PackageUpdateDialogTarget({ targetId, targetName, targetIds, targets, inventory, availableCount, onTargetChange }: Readonly<{ targetId: string; targetName: string | undefined; targetIds: string[]; targets: TargetDto[]; inventory: PackageInventoryDto | undefined; availableCount: number; onTargetChange: (targetId: string) => void }>) {
  const collectedAt = inventory?.collected_at;
  const timestamp = collectedAt ? ` · ${new Date(collectedAt).toLocaleString()}` : "";
  return <div className="flex flex-wrap items-center gap-3 border-b border-[var(--line)] p-4">
    {targetIds.length > 1 ? <label className="min-w-56 flex-1"><span>Ziel</span><select value={targetId} onChange={(event) => onTargetChange(event.target.value)}>{targetIds.map((id) => <option key={id} value={id}>{targets.find((item) => item.id === id)?.name ?? id}</option>)}</select></label> : <strong>{targetName ?? "Kein Ziel ausgewählt"}</strong>}
    <span className="text-xs text-[var(--muted)]">{availableCount} Updates im letzten Inventar{timestamp}</span>
  </div>;
}

function PackageUpdateDialogList({ targetId, inventory, availableUpdates, selectedPackages, onPackageSelected }: Readonly<{ targetId: string; inventory: PackageInventoryDto | undefined; availableUpdates: PackageInventoryDto["packages"]; selectedPackages: string[]; onPackageSelected: (packageName: string, selected: boolean) => void }>) {
  if (availableUpdates.length === 0) return <div className="grid min-h-36 place-items-center gap-2 border border-dashed border-[var(--line)] px-4 text-center text-sm text-[var(--muted)]">{inventory ? <span>Im letzten Paketinventar wurden keine verfügbaren Updates gefunden.</span> : <><span>Für dieses Ziel ist noch kein Paketinventar verfügbar.</span>{targetId ? <Link className="font-semibold text-lxcup-primary hover:underline" to={`/targets/${targetId}/packages`}>Paketinventar öffnen →</Link> : null}</>}</div>;
  return <ul className="m-0 grid min-h-0 list-none gap-2 overflow-y-auto p-4">{availableUpdates.map((item) => <li key={`${targetId}-${item.name}`}><label className="grid cursor-pointer grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-2.5 hover:border-[var(--primary)]"><input className="h-4 w-4 accent-lxcup-primary" type="checkbox" checked={selectedPackages.includes(item.name)} onChange={(event) => onPackageSelected(item.name, event.target.checked)} /><span className="min-w-0"><strong className="block truncate text-sm text-[var(--ink)]">{item.name}</strong><span className="text-xs text-[var(--muted)]">{item.installed_version} → {item.candidate_version}</span></span><span className="text-xs text-[var(--muted)]">{item.architecture ?? ""}</span></label></li>)}</ul>;
}

function PackageUpdateDialogFooter({ selectedCount, onClearSelection, onClose }: Readonly<{ selectedCount: number; onClearSelection: () => void; onClose: () => void }>) {
  const selectedLabel = selectedPackageLabel(selectedCount);
  return <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--line)] p-4"><span className="text-xs text-[var(--muted)]">{selectedLabel}</span><div className="flex gap-2"><button className="border border-[var(--line)] px-3 py-2 text-xs font-semibold hover:bg-[var(--paper-muted)]" type="button" onClick={onClearSelection}>Auswahl leeren (alle)</button><button className="border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white hover:bg-blue-700" type="button" onClick={onClose}>Übernehmen</button></div></footer>;
}

function selectedPackageLabel(selectedCount: number) {
  if (selectedCount === 0) return "Keine Auswahl: alle Updates";
  const unit = selectedCount === 1 ? "Paket" : "Pakete";
  return `${selectedCount} ${unit} für dieses Ziel ausgewählt`;
}

function workflowConfirmationLabel(mode: AnsibleExecutionMode, targetCount: number) {
  const targets = targetCount === 1 ? "Ziel" : "alle ausgewählten Ziele";
  if (mode === "apply") return `Ich bestätige ${targets}, Umfang und Risiko dieser Änderung.`;
  if (mode === "plan") return `Ich bestätige ${targets} und den Umfang dieser Vorschau.`;
  return `Ich bestätige die Prüfung für ${targets}.`;
}

function workflowTargetKindLabel(kind: string) {
  if (kind === "lxc") return "LXC-Container";
  if (kind === "linux_server") return "Linux-Server";
  if (kind === "windows_server") return "Windows-System";
  return kind;
}

function workflowTargetKindShortLabel(kind: string) {
  if (kind === "lxc") return "LXC";
  if (kind === "linux_server") return "Linux";
  if (kind === "windows_server") return "Win";
  return kind.slice(0, 4);
}

function workflowHistoryDescription(targetId: string | null, targets: TargetDto[]) {
  if (!targetId) return "Nachvollziehbare Ausführungen und ihre Ergebnisse.";
  const targetName = targets.find((target) => target.id === targetId)?.name ?? "dieses Ziel";
  return `Ausführungen für ${targetName}.`;
}

function canSubmitWorkflow({ targetIds, allTargetsExist, isModifyingOperation, confirmed, operation, policyId, policyTargetIds, mode, approvedPlanJobIds, packagePlansByTarget, supportedModes }: {
  targetIds: string[];
  allTargetsExist: boolean;
  isModifyingOperation: boolean;
  confirmed: boolean;
  operation: AnsibleOperation;
  policyId: string;
  policyTargetIds: string[];
  mode: AnsibleExecutionMode;
  approvedPlanJobIds: Record<string, string>;
  packagePlansByTarget: Record<string, NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>>;
  supportedModes: AnsibleExecutionMode[];
}) {
  if (!targetIds.length || !allTargetsExist || !supportedModes.includes(mode) || (isModifyingOperation && !confirmed)) return false;
  if (operation !== "update_packages") return true;
  if (!policyId || !targetIds.every((targetId) => policyTargetIds.includes(targetId))) return false;
  if (mode === "plan") return true;
  return mode === "apply" && targetIds.every((targetId) => (packagePlansByTarget[targetId] ?? []).some((job) => job.id === approvedPlanJobIds[targetId]));
}

function JobFailureSummary({ jobId }: Readonly<{ jobId: string }>) {
  const events = useAnsibleJobEvents(jobId, false);
  const failure = events.data?.slice().reverse().find((event) => event.event.kind === "failed");
  if (events.isLoading) return <span className="text-[var(--muted)]">wird geladen…</span>;
  if (failure?.event.code) return <span className="inline-block border border-[var(--error)] px-1.5 py-0.5 text-[11px] font-semibold text-[var(--error)]" title="Fehlercode aus dem Worker-Protokoll">{failure.event.code}</span>;
  if (events.error) return <span className="text-[var(--muted)]">Protokoll nicht verfügbar</span>;
  return <span className="text-[var(--muted)]">kein Fehlercode gemeldet</span>;
}

function workflowSummary(jobs: ReturnType<typeof useAnsibleJobs>["data"]) {
  if (!jobs?.length) return null;
  const counts = jobs.reduce((summary, job) => {
    summary.succeeded += job.status === "succeeded" || job.status === "completed" ? 1 : 0;
    summary.failed += job.status === "failed" || job.status === "reconcile_required" ? 1 : 0;
    summary.active += ["queued", "checking", "planned", "applying"].includes(job.status) ? 1 : 0;
    return summary;
  }, { succeeded: 0, failed: 0, active: 0 });
  return <div className="flex flex-wrap items-center gap-2 border-b border-[var(--line)] px-4 py-2.5 text-xs">
    <span className="font-semibold text-[var(--muted)]">Statusübersicht</span>
    <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2 py-1">{counts.succeeded} erfolgreich</span>
    <span className={cn("border border-[var(--line)] px-2 py-1", counts.failed > 0 ? "border-[var(--error)] bg-[var(--error-soft)] text-[var(--error)]" : "bg-[var(--paper-muted)] text-[var(--muted)]")}>{counts.failed} fehlgeschlagen</span>
    {counts.active > 0 ? <span className="border border-[var(--line)] bg-[var(--primary-soft)] px-2 py-1 text-lxcup-primary">{counts.active} offen oder in Bearbeitung</span> : null}
  </div>;
}

function workflowJobsContent(jobs: ReturnType<typeof useAnsibleJobs>, visibleJobs: typeof jobs.data, targets: NonNullable<ReturnType<typeof useTargets>["data"]>) {
  if (jobs.isLoading) return <p className="text-[var(--muted)]">Lade Workflows…</p>;
  if (jobs.error) return <p className="font-semibold text-[var(--error)]" role="alert">{jobs.error.message}</p>;
  if (visibleJobs === undefined || visibleJobs.length === 0) return <p className="m-4 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Workflows gestartet.</p>;
  return <div className="overflow-x-auto"><table className="min-w-[48rem]"><thead><tr><th className="pl-4">Workflow</th><th>Ressource</th><th>Modus</th><th>Status</th><th className="pr-4">Erstellt</th></tr></thead><tbody>{visibleJobs.map((job) => <tr key={job.id}>
    <td className="pl-4"><Link className="grid gap-1 text-sm font-semibold text-lxcup-primary hover:underline" to={`/workflows/${job.id}`}><span>{workflowOperationLabel(job.operation)}</span><small className="font-normal text-[var(--muted)]">Job · {job.id.slice(0, 8)}</small></Link></td>
    <td>{workflowTarget(job, targets)}</td>
    <td><span className="inline-flex border border-[var(--line)] bg-[var(--paper-muted)] px-2 py-1 text-xs font-medium">{workflowModeLabel(job.mode)}</span></td>
    <td><div className="grid justify-items-start gap-1"><span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", jobStatusBadgeClass(job.status))}>{jobStatusLabel(job.status)}</span>{failureSummary(job.status, job.id)}</div></td>
    <td className="pr-4 text-sm text-[var(--muted)]">{new Date(job.created_at).toLocaleString()}</td>
  </tr>)}</tbody></table></div>;
}

function compareText(left: string, right: string) {
  return left.localeCompare(right);
}

function failureSummary(status: string, jobId: string) {
  if (status === "failed" || status === "reconcile_required") return <JobFailureSummary jobId={jobId} />;
  return null;
}

function workflowTarget(job: { target: { container: number } | { node: string } | { target: string } }, targets: NonNullable<ReturnType<typeof useTargets>["data"]>) {
  if ("container" in job.target) return <span className="text-[var(--muted)]">Container {job.target.container}</span>;
  if ("node" in job.target) return <span className="text-[var(--muted)]">Node {job.target.node}</span>;
  const targetId = (job.target as { target: string }).target;
  if (targetId) {
    const target = targets.find((candidate) => candidate.id === targetId);
    if (!target) return <span title={targetId}>Ziel {targetId.slice(0, 8)}</span>;
    return <span className="grid gap-0.5" title={`${target.name} · ${target.address}`}><strong className="font-medium">{target.name}</strong><small className="text-[var(--muted)]">{target.kind} · {target.address}</small></span>;
  }
  return <span className="text-[var(--muted)]">Unbekanntes Ziel</span>;
}

function workflowOperationLabel(operation: AnsibleOperation) {
  return operations.find((item) => item.value === operation)?.label ?? operation.replaceAll("_", " ");
}

function workflowModeLabel(mode: AnsibleExecutionMode) {
  return modes.find((item) => item.value === mode)?.label ?? mode;
}

function jobProcessingLabel(status: string) {
  if (status === "queued") return "wartet auf Worker";
  if (["checking", "planned", "applying"].includes(status)) return "wird bearbeitet";
  if (status === "reconcile_required") return "Abgleich erforderlich";
  return "beendet";
}
