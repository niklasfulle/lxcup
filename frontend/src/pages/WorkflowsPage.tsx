import { cn, ui } from "../ui";
import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { createAnsibleJob, type AnsibleExecutionMode, type AnsibleOperation, type CreateAnsibleJobRequest } from "../api";
import { queryKeys, useAnsibleJobEvents, useAnsibleJobs, useTargets, useUpdatePolicies } from "../queries";

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
  { value: "reconcile", label: "Reconcile", effect: "Abgleich", description: "Gleicht einen unterbrochenen oder unklaren Workflow mit dem tatsächlichen Zielzustand ab und entscheidet danach über den weiteren Job-Status." },
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
    idempotency_key: operation === "update_packages" && mode === "plan" && policyId ? `package-plan:${policyId}:${crypto.randomUUID()}` : operation === "update_packages" && mode === "apply" && approvedPlanJobId ? `package-apply:${approvedPlanJobId}:${crypto.randomUUID()}` : crypto.randomUUID(),
    confirmed,
    ...(operation === "update_packages" ? { policy_id: policyId, approved_plan_job_id: approvedPlanJobId } : {}),
  };
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
  const isMutating = operation !== "health_check";
  const requestedPackages = packages.split(",").map((value) => value.trim()).filter(Boolean).sort().join(",");
  const packagePlans = (jobs.data ?? []).filter((job) => job.operation === "update_packages" && job.mode === "plan" && job.status === "succeeded" && "target" in job.target && job.target.target === targetId && job.update_policy_id === policyId && [...(job.package_names ?? [])].sort().join(",") === requestedPackages);
  const canSubmit = Boolean(targetId) && (isMutating === false || confirmed) && (operation !== "update_packages" || (packages.trim().length > 0 && policyId !== "" && (mode === "plan" || (mode === "apply" && packagePlans.some((job) => job.id === approvedPlanJobId)))));

  function submit(event: Readonly<{ preventDefault: () => void }>) {
    event.preventDefault();
    if (targetId === "" || canSubmit === false) return;
    mutation.mutate(buildWorkflowRequest(targetId, operation, mode, packages, confirmed, policyId || undefined, approvedPlanJobId || undefined));
  }

  return (
    <>
      <header className={ui.pageHeader}><div><p className={ui.eyebrow}>Ansible Worker</p><h1>Automatisierung</h1><p className={ui.muted}>Freigegebene Workflows für Agenten und Paketpläne.</p></div></header>
      <section className={ui.panel}>
        <form onSubmit={submit}>
          <div className={ui.workflowGrid}>
            <label title="Das registrierte Ziel, gegen das der freigegebene Workflow ausgeführt wird."><span>Ziel</span>
              <select value={targetId} onChange={(event) => setTargetId(event.target.value)}>
                <option value="">Ziel auswählen</option>
                {(targets.data ?? []).map((target) => <option key={target.id} value={target.id}>{target.name} · {target.kind} · {target.address}</option>)}
              </select>
            </label>
            <label title="Die erlaubte, fest registrierte Aktion des Workers."><span>Operation</span>
              <select value={operation} onChange={(event) => { const next = event.target.value as AnsibleOperation; setOperation(next); if (next === "update_packages") setMode("plan"); }}>
                {operations.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
            <label title="Check prüft, Plan erstellt eine Vorschau, Apply führt aus und Reconcile gleicht einen unklaren Zustand ab."><span>Modus</span>
              <select value={mode} onChange={(event) => setMode(event.target.value as AnsibleExecutionMode)} aria-describedby="mode-help">
              {modes.filter((item) => operation !== "update_packages" || item.value === "plan" || item.value === "apply").map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
          </div>
          {operation === "update_packages" ? <label className={ui.workflowField} title="Nur Pakete verwenden, die bereits durch einen Update-Plan validiert wurden."><span>Validierte Pakete aus dem Plan</span>
            <input value={packages} onChange={(event) => setPackages(event.target.value)} placeholder="z. B. nginx,curl" aria-describedby="package-help" />
            <small id="package-help" className={ui.muted}>Die Policy begrenzt die Pakete. Plan prüft ohne Änderung; Apply benötigt denselben erfolgreichen Plan.</small>
          </label> : null}
          {operation === "update_packages" ? <div className={ui.workflowGrid}>
            <label><span>Freigegebene Update-Policy</span><select value={policyId} onChange={(event) => setPolicyId(event.target.value)}><option value="">Policy auswählen</option>{(policies.data ?? []).filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id} · max. Risiko {policy.maximum_risk}</option>)}</select></label>
            {mode === "apply" ? <label><span>Erfolgreicher Plan zur Bestätigung</span><select value={approvedPlanJobId} onChange={(event) => setApprovedPlanJobId(event.target.value)}><option value="">Plan auswählen</option>{packagePlans.map((job) => <option key={job.id} value={job.id}>{job.id} · {new Date(job.updated_at).toLocaleString()}</option>)}</select></label> : null}
            {mode === "plan" ? <p className={cn(ui.callout, ui.calloutInfo)}>Nach erfolgreichem Plan kannst du denselben Ziel- und Paketumfang mit Apply ausdrücklich bestätigen.</p> : null}
          </div> : null}
          <div className={ui.workflowSummary}><span>Risiko: <strong>{selectedOperation?.risk}</strong></span><span>Playbook und Secret-Auflösung kommen aus der Registry.</span></div>
          {selectedMode ? <div id="mode-help" className={cn(ui.callout, ui.calloutInfo)} role="note"><strong>{selectedMode.label}: {selectedMode.effect}</strong><p>{selectedMode.description}</p></div> : null}
          {selectedTarget?.state === "pending" ? <div className={cn(ui.callout, ui.calloutInfo)}><strong>Onboarding für {selectedTarget.name}</strong><p>Dieses Ziel wartet noch auf seinen Agenten. Mit „Agent installieren“ startest du den nächsten nachvollziehbaren Schritt.</p></div> : null}
          {isMutating ? <label className={ui.confirmField}><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} /> Ich bestätige Ziel, Umfang und Risiko dieser Änderung.</label> : null}
          <button className={ui.primaryButton} type="submit" disabled={canSubmit === false || mutation.isPending}>{mutation.isPending ? "Wird gestartet…" : "Workflow starten"}</button>
          {mutation.error ? <p className={ui.errorState} role="alert">{mutation.error.message}</p> : null}
          {mutation.data ? <output className={ui.successState}>Job angenommen: <Link to={`/workflows/${mutation.data.id}`}>{mutation.data.id}</Link> · Status {mutation.data.status}</output> : null}
        </form>
      </section>
      <section className={ui.panel}>
        <div className={ui.sectionHeading}><div><h2>Workflow-Protokolle</h2><p className={ui.muted}>{targetFilter ? `Jobs für ${targets.data?.find((target) => target.id === targetFilter)?.name ?? "dieses Ziel"}.` : "Alle Jobs mit ihrem audit-sicheren Ausführungsprotokoll."}</p></div><span className={ui.muted}>{visibleJobs?.length ?? 0} Jobs</span></div>
        <div className={cn(ui.callout, ui.calloutInfo)}><strong>Ausführungszustand</strong><p><b>queued</b> bedeutet: Job wartet, er wird noch nicht bearbeitet. Erst <b>checking</b>, <b>planned</b> oder <b>applying</b> bedeutet, dass ein Worker aktiv arbeitet. In diesem Entwicklungs-Stack ist derzeit kein ausführender Ansible-Worker gestartet.</p></div>
        {workflowJobsContent(jobs, visibleJobs)}
      </section>
    </>
  );
}

function JobFailureSummary({ jobId }: Readonly<{ jobId: string }>) {
  const events = useAnsibleJobEvents(jobId, false);
  const failure = events.data?.slice().reverse().find((event) => event.event.kind === "failed");
  if (events.isLoading) return <span className={ui.muted}>wird geladen…</span>;
  if (failure?.event.code) return <span className={ui.failureCode} title="Fehlercode aus dem Worker-Protokoll">{failure.event.code}</span>;
  if (events.error) return <span className={ui.muted}>Protokoll nicht verfügbar</span>;
  return <span className={ui.muted}>kein Fehlercode gemeldet</span>;
}

function workflowJobsContent(jobs: ReturnType<typeof useAnsibleJobs>, visibleJobs: typeof jobs.data) {
  if (jobs.isLoading) return <p className={ui.muted}>Lade Workflows…</p>;
  if (jobs.error) return <p className={ui.errorState} role="alert">{jobs.error.message}</p>;
  if (visibleJobs === undefined || visibleJobs.length === 0) return <p className={ui.emptyState}>Noch keine Workflows gestartet.</p>;
  return <div className={ui.tableWrap}><table><thead><tr><th>Workflow</th><th>Ziel</th><th>Modus</th><th>Status</th><th>Fehlerursache</th><th>Bearbeitung</th><th>Gestartet</th></tr></thead><tbody>{visibleJobs.map((job) => <tr key={job.id}><td><Link className={ui.textLink} to={`/workflows/${job.id}`}>{job.operation.replaceAll("_", " ")}</Link></td><td>{workflowTarget(job)}</td><td>{job.mode}</td><td><span className={cn(ui.statusBadge, workflowStatusClass(job.status) === "success" ? ui.statusSuccess : workflowStatusClass(job.status) === "neutral" ? ui.statusNeutral : ui.statusPending)}>{job.status}</span></td><td>{failureSummary(job.status, job.id)}</td><td>{jobProcessingLabel(job.status)}</td><td>{new Date(job.created_at).toLocaleString()}</td></tr>)}</tbody></table></div>;
}

function workflowStatusClass(status: string) {
  if (status === "succeeded") return "success";
  if (status === "failed" || status === "aborted") return "neutral";
  return "pending";
}

function failureSummary(status: string, jobId: string) {
  if (status === "failed" || status === "reconcile_required") return <JobFailureSummary jobId={jobId} />;
  return <span className={ui.muted}>—</span>;
}

function workflowTarget(job: { target: { container: number } | { node: string } | { target: string } }) {
  if ("container" in job.target) return `Container ${job.target.container}`;
  if ("node" in job.target) return `Node ${job.target.node}`;
  return `Ziel ${job.target.target}`;
}

function jobProcessingLabel(status: string) {
  if (status === "queued") return "wartet auf Worker";
  if (["checking", "planned", "applying"].includes(status)) return "wird bearbeitet";
  if (status === "reconcile_required") return "Abgleich erforderlich";
  return "beendet";
}
