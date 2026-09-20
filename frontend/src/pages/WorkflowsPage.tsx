import { useMemo, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createAnsibleJob, type AnsibleExecutionMode, type AnsibleOperation, type CreateAnsibleJobRequest } from "../api";
import { queryKeys, useContainers } from "../queries";

const operations: Array<{ value: AnsibleOperation; label: string; risk: string }> = [
  { value: "deploy_agent", label: "Agent installieren", risk: "Ändernd" },
  { value: "update_agent", label: "Agent aktualisieren", risk: "Ändernd" },
  { value: "repair_agent", label: "Agent reparieren", risk: "Ändernd" },
  { value: "update_packages", label: "Pakete aktualisieren", risk: "Ändernd" },
  { value: "health_check", label: "Healthcheck", risk: "Nur lesend" },
];

const modes: Array<{ value: AnsibleExecutionMode; label: string }> = [
  { value: "check", label: "Check" },
  { value: "plan", label: "Plan / Dry-Run" },
  { value: "apply", label: "Apply" },
  { value: "reconcile", label: "Reconcile" },
];

export function buildWorkflowRequest(
  containerId: number,
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
    container_id: containerId,
    mode,
    parameters,
    idempotency_key: crypto.randomUUID(),
    confirmed,
  };
}

export function WorkflowsPage() {
  const containers = useContainers();
  const queryClient = useQueryClient();
  const [containerId, setContainerId] = useState<number | undefined>();
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
  const isMutating = operation !== "health_check";
  const canSubmit = Boolean(containerId) && (!isMutating || confirmed) && (operation !== "update_packages" || packages.trim().length > 0);

  function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!containerId || !canSubmit) return;
    mutation.mutate(buildWorkflowRequest(containerId, operation, mode, packages, confirmed));
  }

  return (
    <>
      <header className="page-header"><div><p className="eyebrow">Ansible Worker</p><h1>Automatisierung</h1><p className="muted">Freigegebene Workflows für Agenten und Paketpläne.</p></div></header>
      <section className="panel workflow-panel">
        <form onSubmit={submit}>
          <div className="workflow-grid">
            <label> Zielcontainer
              <select value={containerId ?? ""} onChange={(event) => setContainerId(event.target.value ? Number(event.target.value) : undefined)}>
                <option value="">Container auswählen</option>
                {(containers.data ?? []).map((container) => <option key={container.id} value={container.id}>{container.name} · {container.id}</option>)}
              </select>
            </label>
            <label> Operation
              <select value={operation} onChange={(event) => setOperation(event.target.value as AnsibleOperation)}>
                {operations.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
            <label> Modus
              <select value={mode} onChange={(event) => setMode(event.target.value as AnsibleExecutionMode)}>
                {modes.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}
              </select>
            </label>
          </div>
          {operation === "update_packages" ? <label className="workflow-field"> Validierte Pakete aus dem Plan
            <input value={packages} onChange={(event) => setPackages(event.target.value)} placeholder="z. B. nginx,curl" aria-describedby="package-help" />
            <small id="package-help" className="muted">Die API akzeptiert ausschließlich bereits validierte Planpakete.</small>
          </label> : null}
          <div className="workflow-summary"><span>Risiko: <strong>{selectedOperation?.risk}</strong></span><span>Playbook und Secret-Auflösung kommen aus der Registry.</span></div>
          {isMutating ? <label className="confirm-field"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} /> Ich bestätige Ziel, Umfang und Risiko dieser Änderung.</label> : null}
          <button className="primary-button" type="submit" disabled={!canSubmit || mutation.isPending}>{mutation.isPending ? "Wird gestartet…" : "Workflow starten"}</button>
          {mutation.error ? <p className="error-state" role="alert">{mutation.error.message}</p> : null}
          {mutation.data ? <p className="success-state" role="status">Job angenommen: {mutation.data.id} · Status {mutation.data.status}</p> : null}
        </form>
      </section>
    </>
  );
}
