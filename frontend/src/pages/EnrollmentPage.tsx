import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { createAnsibleJob, createEnrollment } from "../api";
import { useContainers, useEnrollment, useTargets } from "../queries";

const steps = [
  ["requested", "Anfrage angenommen"],
  ["discovering", "LXC validieren"],
  ["installing_agent", "Agent ausrollen"],
  ["registering_agent", "Agent registrieren"],
  ["connected", "Health bestätigt"],
] as const;

const stateLabels: Record<string, string> = {
  requested: "Anfrage angenommen",
  discovering: "LXC wird geprüft",
  installing_agent: "Agent wird installiert",
  registering_agent: "Agent wird registriert",
  connected: "Verbunden",
  failed: "Fehlgeschlagen",
  disabled: "Deaktiviert",
};

const stateDescriptions: Record<string, string> = {
  requested: "Die Anfrage ist gespeichert. Der nächste Worker-Schritt wird vorbereitet.",
  discovering: "Die Zielparameter und der aktuelle Containerzustand werden geprüft.",
  installing_agent: "Der freigegebene Agent wird kontrolliert auf dem LXC ausgerollt.",
  registering_agent: "Der Agent wird beim Controller registriert und erhält seine Zielidentität.",
  connected: "Der Agent hat den Heartbeat bestätigt. Das Ziel kann jetzt verwaltet werden.",
  failed: "Der Ablauf wurde angehalten. Die Fehlermeldung steht unten.",
  disabled: "Dieses Enrollment ist deaktiviert.",
};

export function EnrollmentPage() {
  const containers = useContainers();
  const targets = useTargets();
  const [selectedContainer, setSelectedContainer] = useState("");
  const [selectedTarget, setSelectedTarget] = useState("");
  const [enrollmentId, setEnrollmentId] = useState<string>();
  const [submitError, setSubmitError] = useState<string>();
  const [startOnboarding, setStartOnboarding] = useState(true);
  const [healthJobId, setHealthJobId] = useState<string>();
  const healthStartedFor = useRef<string | undefined>(undefined);
  const enrollment = useEnrollment(enrollmentId);
  const available = (containers.data ?? []).filter((container) => container.status === "running");

  async function startEnrollment() {
    if (!selectedContainer || (startOnboarding && !selectedTarget)) return;
    setSubmitError(undefined);
    try {
      const result = await createEnrollment(Number(selectedContainer), selectedTarget || undefined, startOnboarding);
      setEnrollmentId(result.id);
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : "Enrollment konnte nicht gestartet werden.");
    }
  }

  function retry() {
    setEnrollmentId(undefined);
    setSubmitError(undefined);
  }

  const state = enrollment.data?.state;
  const health = useMutation({
    mutationFn: () => createAnsibleJob({ operation: "health_check", container_id: Number(selectedContainer), mode: "check", parameters: { operation: "health_check" }, idempotency_key: `enrollment-health-${enrollmentId}`, confirmed: true }),
    onSuccess: (job) => setHealthJobId(job.id),
  });

  useEffect(() => {
    if (!startOnboarding || state !== "connected" || !enrollmentId || healthStartedFor.current === enrollmentId) return;
    healthStartedFor.current = enrollmentId;
    health.mutate();
  }, [enrollmentId, health, startOnboarding, state]);

  return <>
    <header className="page-header"><div><p className="eyebrow">Aufnahme</p><h1>LXC hinzufügen</h1><p className="muted">Bestehenden laufenden LXC auswählen und den Agenten kontrolliert ausrollen.</p></div></header>
    <section className="panel enrollment-panel">
      {!enrollmentId ? <>
        <label className="workflow-field" htmlFor="enrollment-container">LXC auswählen
          <select id="enrollment-container" value={selectedContainer} onChange={(event) => setSelectedContainer(event.target.value)}>
            <option value="">Bitte auswählen…</option>
            {available.map((container) => <option key={container.id} value={container.id}>{container.name} · VMID {container.id}</option>)}
          </select>
        </label>
        {startOnboarding ? <label className="workflow-field" htmlFor="enrollment-target">Registriertes Ziel für den Agenten
          <select id="enrollment-target" value={selectedTarget} onChange={(event) => setSelectedTarget(event.target.value)}>
            <option value="">Bitte Ziel auswählen…</option>
            {(targets.data ?? []).filter((target) => target.state !== "disabled").map((target) => <option key={target.id} value={target.id}>{target.name} · {target.address}</option>)}
          </select>
          <span className="muted">Das Ziel liefert SSH-Zugang, Host-Key und Agent-Token für den vollständigen Deployment-Lauf.</span>
        </label> : null}
        <label className="confirm-field"><input type="checkbox" checked={startOnboarding} onChange={(event) => setStartOnboarding(event.target.checked)} /> Onboarding starten: Agent installieren und nach erfolgreicher Verbindung einen Healthcheck ausführen.</label>
        {!containers.isLoading && available.length === 0 ? <p className="muted">Keine laufenden, noch auswählbaren LXCs entdeckt.</p> : null}
        {submitError ? <p className="error-state" role="alert">{submitError}</p> : null}
        <button className="primary-button" type="button" disabled={!selectedContainer || (startOnboarding && !selectedTarget)} onClick={() => void startEnrollment()}>Enrollment starten</button>
      </> : <>
        <div className="section-heading"><div><h2>Enrollment-Fortschritt</h2><p className="muted">Container <Link to={`/containers/${selectedContainer}`}>VMID {selectedContainer}</Link></p></div><span className={`status-badge ${state === "connected" ? "success" : state === "failed" ? "neutral" : "pending"}`}>{state ? stateLabels[state] ?? state : "Wird geladen…"}</span></div>
        <ol className="enrollment-steps">{steps.map(([step, label]) => <li key={step} className={state === step ? "current" : steps.findIndex(([value]) => value === (state ?? "requested")) >= steps.findIndex(([value]) => value === step) ? "complete" : ""}><span aria-hidden="true">{state === step ? "…" : "✓"}</span>{label}</li>)}</ol>
        {state ? <div className="callout info"><strong>Aktueller Schritt</strong><p>{stateDescriptions[state] ?? "Der Status wird vom Controller aktualisiert."}</p></div> : null}
        {enrollment.isLoading ? <p className="muted">Status wird geladen…</p> : null}
        {enrollment.error ? <p className="error-state" role="alert">{enrollment.error.message}</p> : null}
        {state === "failed" ? <><p className="error-state" role="alert">{enrollment.data?.failure_reason ?? "Der Agent konnte nicht registriert werden."}</p><button className="primary-button" type="button" onClick={retry}>Erneut versuchen</button></> : null}
        {state === "connected" ? <div className="success-state">Agent verbunden. {startOnboarding ? health.isPending ? "Der Healthcheck wird gestartet…" : healthJobId ? <Link to={`/workflows/${healthJobId}`}>Healthcheck-Protokoll öffnen</Link> : "Der Healthcheck wird vorbereitet." : <Link to={`/containers/${selectedContainer}`}>Health und Metriken öffnen</Link>}{health.error ? <p className="error-state">{health.error.message}</p> : null}</div> : null}
      </>}
    </section>
  </>;
}
