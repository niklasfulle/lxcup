import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createAnsibleJob, createTarget, listSecrets, type TargetKind, type TargetTransport } from "../api";
import { queryKeys, useAnsibleJob, useTargets } from "../queries";
import { TargetLifecycle } from "../components/TargetLifecycle";

const kinds: Array<{ value: TargetKind; label: string; transport: TargetTransport }> = [
  { value: "lxc", label: "LXC", transport: "ssh" },
  { value: "linux_server", label: "Linux-Server", transport: "ssh" },
  { value: "windows_server", label: "Windows-System", transport: "winrm" },
];

export function TargetsPage() {
  const queryClient = useQueryClient();
  const targets = useTargets();
  const secrets = useQuery({ queryKey: ["secrets"], queryFn: ({ signal }) => listSecrets(signal) });
  const [name, setName] = useState("");
  const [address, setAddress] = useState("");
  const [kind, setKind] = useState<TargetKind>("lxc");
  const [credentialSecret, setCredentialSecret] = useState("");
  const [agentSecret, setAgentSecret] = useState("");
  const [createdTargetId, setCreatedTargetId] = useState<string>();
  const [startOnboarding, setStartOnboarding] = useState(true);
  const [deploymentJobId, setDeploymentJobId] = useState<string>();
  const [healthJobId, setHealthJobId] = useState<string>();
  const healthStartedFor = useRef<string>();
  const selectedKind = kinds.find((item) => item.value === kind)!;
  const activeSecrets = (secrets.data ?? []).filter((item) => item.metadata.status === "active");

  const deployment = useMutation({
    mutationFn: (targetId: string) => createAnsibleJob({ operation: "deploy_agent", target_id: targetId, mode: "apply", parameters: { operation: "deploy_agent", agent_version: "0.1.0" }, idempotency_key: `onboarding-deploy-${targetId}`, confirmed: true }),
    onSuccess: (job) => { setDeploymentJobId(job.id); void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }); },
  });
  const deploymentJob = useAnsibleJob(deploymentJobId);
  const health = useMutation({
    mutationFn: (targetId: string) => createAnsibleJob({ operation: "health_check", target_id: targetId, mode: "check", parameters: { operation: "health_check" }, idempotency_key: `onboarding-health-${targetId}`, confirmed: true }),
    onSuccess: (job) => { setHealthJobId(job.id); void queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }); },
  });
  const create = useMutation({
    mutationFn: () => createTarget({
      name,
      address,
      kind,
      transport: selectedKind.transport,
      credential_secret_ref: credentialSecret,
      agent_secret_ref: agentSecret,
    }),
    onSuccess: (target) => {
      setCreatedTargetId(target.id);
      setDeploymentJobId(undefined);
      setHealthJobId(undefined);
      setName("");
      setAddress("");
      void queryClient.invalidateQueries({ queryKey: queryKeys.targets });
      if (startOnboarding) deployment.mutate(target.id);
    },
  });

  const createdTarget = targets.data?.find((target) => target.id === createdTargetId) ?? create.data;
  const pendingTargets = (targets.data ?? []).filter((target) => target.state === "pending" && target.id !== createdTargetId);

  useEffect(() => {
    if (!startOnboarding || !createdTarget || !deploymentJob.data || deploymentJob.data.status !== "succeeded" || createdTarget.state !== "managed" || healthStartedFor.current === createdTarget.id) return;
    healthStartedFor.current = createdTarget.id;
    health.mutate(createdTarget.id);
  }, [createdTarget, deploymentJob.data, health, startOnboarding]);

  return (
    <>
      <header className="page-header">
        <div>
          <p className="eyebrow">Inventar</p>
          <h1>Ziele</h1>
          <p className="muted">Registrierte Ziele und ihr tatsächlicher Onboarding-Fortschritt.</p>
        </div>
        <Link className="secondary-button" to="/enrollments/new">LXC aus Container auswählen</Link>
      </header>

      <section className="panel workflow-panel">
        <div className="section-heading">
          <div>
            <h2>Ziel registrieren</h2>
            <p className="muted">Für ein bereits entdecktes LXC empfehlen wir den geführten Onboarding-Dialog.</p>
          </div>
          <Link className="text-link" to="/enrollments/new">Zum LXC-Onboarding →</Link>
        </div>
        <form onSubmit={(event) => { event.preventDefault(); create.mutate(); }}>
          <div className="workflow-grid">
            <label>Name<input value={name} onChange={(event) => setName(event.target.value)} required /></label>
            <label>Typ<select value={kind} onChange={(event) => setKind(event.target.value as TargetKind)}>{kinds.map((item) => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label>
            <label>Adresse<input value={address} onChange={(event) => setAddress(event.target.value)} placeholder="IP oder DNS-Name" required /></label>
            <label>Deployment-Secret<select value={credentialSecret} onChange={(event) => setCredentialSecret(event.target.value)} required><option value="">Secret auswählen</option>{activeSecrets.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select></label>
            <label>Agent-Token<select value={agentSecret} onChange={(event) => setAgentSecret(event.target.value)} required><option value="">Secret auswählen</option>{activeSecrets.map((item) => <option key={item.metadata.metadata.id} value={item.metadata.metadata.id}>{item.metadata.metadata.name}</option>)}</select></label>
            <label>Transport<input value={selectedKind.transport.toUpperCase()} readOnly /></label>
          </div>
          <label className="confirm-field"><input type="checkbox" checked={startOnboarding} onChange={(event) => setStartOnboarding(event.target.checked)} /> Onboarding direkt starten: Agent installieren und nach erfolgreichem Heartbeat einen Healthcheck ausführen.</label>
          <button className="primary-button" type="submit" disabled={create.isPending || !credentialSecret || !agentSecret}>
            {create.isPending ? "Wird angelegt…" : "Ziel registrieren"}
          </button>
          {create.error ? <p className="error-state" role="alert">{create.error.message}</p> : null}
        </form>
      </section>

      {createdTarget ? <TargetLifecycle target={createdTarget} /> : null}
      {createdTarget && startOnboarding ? <section className="panel"><div className="section-heading"><div><h2>Onboarding-Aktivitäten</h2><p className="muted">Jeder Schritt wird als eigener Workflow mit eigenem Protokoll geführt.</p></div></div><div className="onboarding-activities"><div><strong>1. Agent installieren</strong>{deployment.isPending ? <span className="muted"> wird erstellt…</span> : deployment.data ? <Link className="text-link" to={`/workflows/${deployment.data.id}`}>Protokoll öffnen →</Link> : null}{deployment.error ? <p className="error-state">{deployment.error.message}</p> : null}</div><div><strong>2. Healthcheck</strong>{health.isPending ? <span className="muted"> wird gestartet…</span> : health.data ? <Link className="text-link" to={`/workflows/${health.data.id}`}>Protokoll öffnen →</Link> : <p className="muted">Startet nach erfolgreicher Agent-Installation und Heartbeat.</p>}{health.error ? <p className="error-state">{health.error.message}</p> : null}</div></div></section> : null}

      {pendingTargets.length ? (
        <section className="panel">
          <div className="section-heading">
            <div>
              <h2>Offene Onboardings</h2>
              <p className="muted">Diese Ziele warten noch auf Agent und Heartbeat.</p>
            </div>
            <span className="status-badge pending">{pendingTargets.length} offen</span>
          </div>
          <div className="lifecycle-list">
            {pendingTargets.map((target) => <TargetLifecycle key={target.id} target={target} />)}
          </div>
        </section>
      ) : null}

      <section className="panel">
        <div className="section-heading">
          <div>
            <h2>Target-Inventar</h2>
            <p className="muted">Alle registrierten Verbindungen</p>
          </div>
          <span className="muted">{targets.data?.length ?? 0} Ziele</span>
        </div>
        {targets.isLoading ? <p className="muted">Lade Ziele…</p> : !targets.data?.length ? <p className="empty-state">Noch keine Ziele angelegt.</p> : (
          <div className="table-wrap">
            <table>
              <thead><tr><th>Name</th><th>Typ</th><th>Adresse</th><th>Transport</th><th>Status</th></tr></thead>
              <tbody>{targets.data.map((target) => (
                <tr key={target.id}>
                  <td>{target.name}</td>
                  <td>{target.kind}</td>
                  <td>{target.address}</td>
                  <td>{target.transport}</td>
                  <td><span className={`status-badge ${target.state === "managed" ? "success" : target.state === "disabled" ? "neutral" : "pending"}`}>{target.state === "managed" ? "Verbunden" : target.state === "disabled" ? "Deaktiviert" : "Pending"}</span></td>
                </tr>
              ))}</tbody>
            </table>
          </div>
        )}
      </section>
    </>
  );
}
