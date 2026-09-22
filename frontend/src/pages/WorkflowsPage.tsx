import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useSearchParams } from "react-router-dom";
import { createAnsibleJob, type AnsibleExecutionMode, type AnsibleOperation, type CreateAnsibleJobRequest } from "../api";
import { queryKeys, useAnsibleJobs, useTargets } from "../queries";

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
): CreateAnsibleJobRequest {
  const parameters: Record<string, unknown> = operation === "update_packages"
    ? { operation, packages: packages.split(",").map((value) => value.trim()).filter(Boolean) }
    : operation === "deploy_agent" || operation === "update_agent"
      ? { operation, agent_version: "0.1.0" }
      : operation === "repair_agent"
        ? { operation }
        : { operation };

  return {
    operation,
    target_id: targetId,
    mode,
    parameters,
    idempotency_key: crypto.randomUUID(),
    confirmed,
  };
}

export function WorkflowsPage() {
  const targets = useTargets();
  const jobs = useAnsibleJobs();
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const [targetId, setTargetId] = useState<string>(() => searchParams.get("target") ?? "");
  const [operation, setOperation] = useState<AnsibleOperation>("health_check");
  const [mode, setMode] = useState<AnsibleExecutionMode>("check");
  const [packages, setPackages] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const mutation = useMutation({
    mutationFn: (request: CreateAnsibleJobRequest) => createAnsibleJob(request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs });
    },
  });
  const selectedOperation = useMemo(() => operations.find((item) => item.value === operation), [operation]);
  const selectedMode = useMemo(() => modes.find((item) => item.value === mode), [mode]);
  const selectedTarget = targets.data?.find((target) => target.id === targetId);
  const isMutating = operation !== "health_check";
  const canSubmit = Boolean(targetId) && (!isMutating || confirmed) && (operation !== "update_packages" || packages.trim().length > 0);

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!targetId || !canSubmit) return;
    mutation.mutate(buildWorkflowRequest(targetId, operation, mode, packages, confirmed));
  }

  return (
    <>
      <header className="page-header"><div><p className="eyebrow">Ansible Worker</p><h1>Automatisierung</h1><p className="muted">Freigegebene Workflows für Agenten und Paketpläne.</p></div></header>
      <section className="panel workflow-panel">
        <form onSubmit={submit}>
          <div className="workflow-grid">
            <label> Ziel
              <select value={targetId} onChange={(event) => setTargetId(event.target.value)}>
                <option value="">Ziel auswählen</option>
                {(targets.data ?? []).map((target) => <option key={target.id} value={target.id}>{target.name} · {target.kind} · {target.address}</option>)}
              </select>
            </label>
            <label> Operation
              <select value={operation} onChange={(event) => setOperation(event.target.value as AnsibleOperation)}>
                {operations.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
            <label> Modus
              <select value={mode} onChange={(event) => setMode(event.target.value as AnsibleExecutionMode)} aria-describedby="mode-help">
                {modes.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
          </div>
          {operation === "update_packages" ? <label className="workflow-field"> Validierte Pakete aus dem Plan
            <input value={packages} onChange={(event) => setPackages(event.target.value)} placeholder="z. B. nginx,curl" aria-describedby="package-help" />
            <small id="package-help" className="muted">Die API akzeptiert ausschließlich bereits validierte Planpakete.</small>
          </label> : null}
          <div className="workflow-summary"><span>Risiko: <strong>{selectedOperation?.risk}</strong></span><span>Playbook und Secret-Auflösung kommen aus der Registry.</span></div>
          {selectedMode ? <div id="mode-help" className="callout info" role="note"><strong>{selectedMode.label}: {selectedMode.effect}</strong><p>{selectedMode.description}</p></div> : null}
          {selectedTarget?.state === "pending" ? <div className="callout info"><strong>Onboarding für {selectedTarget.name}</strong><p>Dieses Ziel wartet noch auf seinen Agenten. Mit „Agent installieren“ startest du den nächsten nachvollziehbaren Schritt.</p></div> : null}
          {isMutating ? <label className="confirm-field"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} /> Ich bestätige Ziel, Umfang und Risiko dieser Änderung.</label> : null}
          <button className="primary-button" type="submit" disabled={!canSubmit || mutation.isPending}>{mutation.isPending ? "Wird gestartet…" : "Workflow starten"}</button>
          {mutation.error ? <p className="error-state" role="alert">{mutation.error.message}</p> : null}
          {mutation.data ? <p className="success-state" role="status">Job angenommen: <Link to={`/workflows/${mutation.data.id}`}>{mutation.data.id}</Link> · Status {mutation.data.status}</p> : null}
        </form>
      </section>
      <section className="panel">
        <div className="section-heading"><div><h2>Workflow-Protokolle</h2><p className="muted">Alle Jobs mit ihrem audit-sicheren Ausführungsprotokoll.</p></div><span className="muted">{jobs.data?.length ?? 0} Jobs</span></div>
        <div className="callout info"><strong>Ausführungszustand</strong><p><b>queued</b> bedeutet: Job wartet, er wird noch nicht bearbeitet. Erst <b>checking</b>, <b>planned</b> oder <b>applying</b> bedeutet, dass ein Worker aktiv arbeitet. In diesem Entwicklungs-Stack ist derzeit kein ausführender Ansible-Worker gestartet.</p></div>
        {jobs.isLoading ? <p className="muted">Lade Workflows…</p> : jobs.error ? <p className="error-state" role="alert">{jobs.error.message}</p> : !jobs.data?.length ? <p className="empty-state">Noch keine Workflows gestartet.</p> : <div className="table-wrap"><table><thead><tr><th>Workflow</th><th>Ziel</th><th>Modus</th><th>Status</th><th>Bearbeitung</th><th>Gestartet</th></tr></thead><tbody>{jobs.data.map((job) => <tr key={job.id}><td><Link className="text-link" to={`/workflows/${job.id}`}>{job.operation.replaceAll("_", " ")}</Link></td><td>{workflowTarget(job)}</td><td>{job.mode}</td><td><span className={`status-badge ${job.status === "succeeded" ? "success" : job.status === "failed" || job.status === "aborted" ? "neutral" : "pending"}`}>{job.status}</span></td><td>{jobProcessingLabel(job.status)}</td><td>{new Date(job.created_at).toLocaleString()}</td></tr>)}</tbody></table></div>}
      </section>
    </>
  );
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
