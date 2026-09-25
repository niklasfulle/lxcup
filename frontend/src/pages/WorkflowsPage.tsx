import { cn } from "../classnames";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { createAnsibleJob, type AnsibleExecutionMode, type AnsibleJobDto, type AnsibleOperation, type CreateAnsibleJobRequest, type TargetDto } from "../api";
import { queryKeys, useAnsibleJobEvents, useAnsibleJobs, useTargets, useUpdatePolicies } from "../queries";
import workflowModeMatrix from "../../workflow-modes.json";

const operationModes = workflowModeMatrix as Record<AnsibleOperation, AnsibleExecutionMode[]>;

const operations: Array<{ value: AnsibleOperation; label: string; risk: string }> = [
  { value: "deploy_agent", label: "Agent installieren", risk: "Ändernd" },
  { value: "update_agent", label: "Agent aktualisieren", risk: "Ändernd" },
  { value: "repair_agent", label: "Agent reparieren", risk: "Ändernd" },
  { value: "update_packages", label: "Pakete aktualisieren", risk: "Ändernd" },
  { value: "health_check", label: "Healthcheck", risk: "Nur lesend" },
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
  packages: string,
  confirmed: boolean,
  policyId?: string,
  approvedPlanJobId?: string,
): CreateAnsibleJobRequest {
  let parameters: Record<string, unknown> = { operation };
  if (operation === "update_packages") {
    parameters = { operation, packages: packages.split(",").map((value) => value.trim()).filter(Boolean) };
  } else if (operation === "deploy_agent" || operation === "update_agent") {
    parameters = { operation, agent_version: "0.2.0" };
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
  const [targetId, setTargetId] = useState<string>(() => targetFilter ?? "");
  const [operation, setOperation] = useState<AnsibleOperation>("health_check");
  const [mode, setMode] = useState<AnsibleExecutionMode>("check");
  const [packages, setPackages] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [policyId, setPolicyId] = useState("");
  const [approvedPlanJobId, setApprovedPlanJobId] = useState("");
  const mutation = useMutation({
    mutationFn: (request: CreateAnsibleJobRequest) => createAnsibleJob(request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs });
    },
  });
  const selectedOperation = useMemo(() => operations.find((item) => item.value === operation), [operation]);
  const selectedMode = useMemo(() => modes.find((item) => item.value === mode), [mode]);
  const selectedTarget = targets.data?.find((target) => target.id === targetId);
  const visibleJobs = jobs.data?.filter((job) => targetFilter === null || ("target" in job.target && job.target.target === targetFilter));
  const isModifyingOperation = operation !== "health_check" && operation !== "collect_package_inventory";
  const requestedPackages = packages.split(",").map((value) => value.trim()).filter(Boolean).sort(compareText).join(",");
  const packagePlans = (jobs.data ?? []).filter((job) => job.operation === "update_packages" && job.mode === "plan" && job.status === "succeeded" && "target" in job.target && job.target.target === targetId && job.update_policy_id === policyId && [...(job.package_names ?? [])].sort(compareText).join(",") === requestedPackages);
  const canSubmit = canSubmitWorkflow({ targetId, isModifyingOperation, confirmed, operation, packages, policyId, mode, approvedPlanJobId, packagePlans, supportedModes: operationModes[operation] });

  function submit(event: Readonly<{ preventDefault: () => void }>) {
    event.preventDefault();
    if (targetId === "" || canSubmit === false) return;
    mutation.mutate(buildWorkflowRequest(targetId, operation, mode, packages, confirmed, policyId || undefined, approvedPlanJobId || undefined));
  }

  return <>
      <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Ansible Worker</p><h1>Automatisierung</h1><p className="text-[var(--muted)]">Freigegebene Workflows für Agenten und Paketpläne.</p></div></header>
      <WorkflowControlPanel
        targets={targets.data ?? []} targetId={targetId} onTargetChange={setTargetId}
        operation={operation} onOperationChange={(next) => { setOperation(next); setMode(operationModes[next][0] ?? "check"); }}
        mode={mode} onModeChange={setMode} packages={packages} onPackagesChange={setPackages}
        policies={policies.data ?? []} policyId={policyId} onPolicyChange={setPolicyId}
        approvedPlanJobId={approvedPlanJobId} onApprovedPlanChange={setApprovedPlanJobId}
        packagePlans={packagePlans} modifying={isModifyingOperation} confirmed={confirmed}
        onConfirmedChange={setConfirmed} selectedTarget={selectedTarget} selectedOperation={selectedOperation}
        selectedMode={selectedMode} pending={mutation.isPending} error={mutation.error}
        createdJob={mutation.data} onSubmit={submit}
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
  targetId: string;
  onTargetChange: (value: string) => void;
  operation: AnsibleOperation;
  onOperationChange: (value: AnsibleOperation) => void;
  mode: AnsibleExecutionMode;
  onModeChange: (value: AnsibleExecutionMode) => void;
  packages: string;
  onPackagesChange: (value: string) => void;
  policies: NonNullable<ReturnType<typeof useUpdatePolicies>["data"]>;
  policyId: string;
  onPolicyChange: (value: string) => void;
  approvedPlanJobId: string;
  onApprovedPlanChange: (value: string) => void;
  packagePlans: NonNullable<ReturnType<typeof useAnsibleJobs>["data"]>;
  modifying: boolean;
  confirmed: boolean;
  onConfirmedChange: (value: boolean) => void;
  selectedTarget: TargetDto | undefined;
  selectedOperation: typeof operations[number] | undefined;
  selectedMode: typeof modes[number] | undefined;
  pending: boolean;
  error: unknown;
  createdJob: AnsibleJobDto | undefined;
  onSubmit: (event: Readonly<{ preventDefault: () => void }>) => void;
}>;

function WorkflowControlPanel(props: WorkflowControlPanelProps) {
  const { targets, targetId, onTargetChange, operation, onOperationChange, mode, onModeChange, packages, onPackagesChange, policies, policyId, onPolicyChange, approvedPlanJobId, onApprovedPlanChange, packagePlans, modifying, confirmed, onConfirmedChange, selectedTarget, selectedOperation, selectedMode, pending, error, createdJob, onSubmit } = props;
  return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)] shadow-sm">
        <form className="space-y-4" onSubmit={onSubmit}>
          <div className="flex flex-wrap items-start justify-between gap-3 border-b border-[var(--line)] pb-3">
            <div>
              <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Workflow-Steuerung</p>
              <h2 className="m-0">Workflow starten</h2>
              <p className="mt-1 text-[var(--muted)]">Wähle Ziel, freigegebene Aktion und Ausführungsmodus.</p>
            </div>
            <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 py-1 text-xs font-semibold text-[var(--muted)]">Registry-gesichert</span>
          </div>
          <div className="grid grid-cols-1 gap-4 rounded border border-[var(--line)] bg-[var(--paper-muted)] p-4 md:grid-cols-2 xl:grid-cols-3">
            <label className="grid content-start gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Das registrierte Ziel, gegen das der freigegebene Workflow ausgeführt wird."><span>Ziel</span>
                <select value={targetId} onChange={(event) => onTargetChange(event.target.value)}>
                <option value="">Ziel auswählen</option>
                {targets.map((target) => <option key={target.id} value={target.id}>{target.name} · {target.kind} · {target.address}</option>)}
              </select>
            </label>
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
          {operation === "update_packages" ? <label className="grid max-w-lg gap-1.5 text-xs font-semibold text-[var(--muted)]" title="Nur Pakete verwenden, die bereits durch einen Update-Plan validiert wurden."><span>Validierte Pakete aus dem Plan</span>
            <input value={packages} onChange={(event) => onPackagesChange(event.target.value)} placeholder="z. B. nginx,curl" aria-describedby="package-help" />
            <small id="package-help" className="text-[var(--muted)]">Die Policy begrenzt die Pakete. Plan prüft ohne Änderung; Apply benötigt denselben erfolgreichen Plan.</small>
          </label> : null}
          {operation === "update_packages" ? <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
            <label><span>Freigegebene Update-Policy</span><select value={policyId} onChange={(event) => onPolicyChange(event.target.value)}><option value="">Policy auswählen</option>{policies.filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id} · max. Risiko {policy.maximum_risk}</option>)}</select></label>
            {mode === "apply" ? <label><span>Erfolgreicher Plan zur Bestätigung</span><select value={approvedPlanJobId} onChange={(event) => onApprovedPlanChange(event.target.value)}><option value="">Plan auswählen</option>{packagePlans.map((job) => <option key={job.id} value={job.id}>{job.id} · {new Date(job.updated_at).toLocaleString()}</option>)}</select></label> : null}
            {mode === "plan" ? <p className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}>Nach erfolgreichem Plan kannst du denselben Ziel- und Paketumfang mit Apply ausdrücklich bestätigen.</p> : null}
          </div> : null}
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(16rem,0.8fr)]">
            {selectedMode ? <div id="mode-help" className="border border-[var(--line)] bg-[var(--primary-soft)] p-3 text-[var(--ink)]" role="note"><div className="mb-1 flex flex-wrap items-center gap-2"><strong>{selectedMode.label}: {selectedMode.effect}</strong><span className="border border-[var(--line)] bg-[var(--panel)] px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-[var(--muted)]">{selectedOperation?.risk}</span></div><p className="m-0 text-sm text-[var(--muted)]">{selectedMode.description}</p></div> : null}
            <div className="flex flex-col justify-center gap-1 border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-2 text-sm"><strong className="text-xs uppercase tracking-wide text-[var(--muted)]">Sicherheitsrahmen</strong><span>Playbook und Secret-Auflösung kommen ausschließlich aus der Registry.</span></div>
          </div>
          {selectedTarget?.state === "pending" ? <output className="block border border-[#bad0fa] bg-[var(--primary-soft)] p-3 text-[var(--ink)]"><strong>Onboarding für {selectedTarget.name}</strong><span className="mt-1 block text-sm text-[var(--muted)]">Dieses Ziel wartet noch auf seinen Agenten. Mit „Agent installieren“ startest du den nächsten nachvollziehbaren Schritt.</span></output> : null}
          <div className="flex flex-col gap-3 border-t border-[var(--line)] pt-4 sm:flex-row sm:items-center sm:justify-between">
            {modifying ? <label className="flex items-center gap-2 text-sm font-medium"><input className="h-4 w-4 accent-lxcup-primary" type="checkbox" checked={confirmed} onChange={(event) => onConfirmedChange(event.target.checked)} /> {workflowConfirmationLabel(mode)}</label> : <span className="text-xs text-[var(--muted)]">Dieser Workflow führt keine freigegebene Änderung aus.</span>}
            <button className="inline-flex min-h-10 shrink-0 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-5 py-2 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={!canSubmitWorkflow({ targetId, isModifyingOperation: modifying, confirmed, operation, packages, policyId, mode, approvedPlanJobId, packagePlans, supportedModes: operationModes[operation] }) || pending}>{pending ? "Wird gestartet…" : "Workflow starten"}<span aria-hidden="true">→</span></button>
          </div>
          {error instanceof Error ? <p className="m-0 border border-[var(--error)] bg-[var(--error-soft)] px-3 py-2 font-semibold text-[var(--error)]" role="alert">Workflow konnte nicht gestartet werden: {error.message}</p> : null}
          {createdJob ? <output className="flex flex-wrap items-center justify-between gap-2 border border-[#b7e7d0] bg-[var(--success-soft)] px-3 py-2.5 text-sm text-[var(--success)]"><span><strong>Workflow angenommen</strong><span className="ml-2 text-[var(--muted)]">{createdJob.id} · {jobStatusLabel(createdJob.status)}</span></span><Link className="font-semibold underline underline-offset-2" to={`/workflows/${createdJob.id}`}>Protokoll öffnen →</Link></output> : null}
        </form>
      </section>;
}

function workflowConfirmationLabel(mode: AnsibleExecutionMode) {
  if (mode === "apply") return "Ich bestätige Ziel, Umfang und Risiko dieser Änderung.";
  if (mode === "plan") return "Ich bestätige Ziel und Umfang dieser Vorschau.";
  return "Ich bestätige die Prüfung dieses Ziels.";
}

function workflowHistoryDescription(targetId: string | null, targets: TargetDto[]) {
  if (!targetId) return "Nachvollziehbare Ausführungen und ihre Ergebnisse.";
  const targetName = targets.find((target) => target.id === targetId)?.name ?? "dieses Ziel";
  return `Ausführungen für ${targetName}.`;
}

function canSubmitWorkflow({ targetId, isModifyingOperation, confirmed, operation, packages, policyId, mode, approvedPlanJobId, packagePlans, supportedModes }: {
  targetId: string;
  isModifyingOperation: boolean;
  confirmed: boolean;
  operation: AnsibleOperation;
  packages: string;
  policyId: string;
  mode: AnsibleExecutionMode;
  approvedPlanJobId: string;
  packagePlans: import("../api").AnsibleJobDto[];
  supportedModes: AnsibleExecutionMode[];
}) {
  if (!targetId || !supportedModes.includes(mode) || (isModifyingOperation && !confirmed)) return false;
  if (operation !== "update_packages") return true;
  if (!packages.trim() || !policyId) return false;
  if (mode === "plan") return true;
  return mode === "apply" && packagePlans.some((job) => job.id === approvedPlanJobId);
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
