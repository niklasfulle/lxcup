import { useState } from "react";
import { Link } from "react-router-dom";
import { createEnrollment } from "../api";
import { useContainers, useEnrollment } from "../queries";

const steps = [
  ["requested", "Anfrage angenommen"],
  ["discovering", "LXC validieren"],
  ["installing_agent", "Agent ausrollen"],
  ["registering_agent", "Agent registrieren"],
  ["connected", "Health bestätigt"],
] as const;

export function EnrollmentPage() {
  const containers = useContainers();
  const [selectedContainer, setSelectedContainer] = useState("");
  const [enrollmentId, setEnrollmentId] = useState<string>();
  const [submitError, setSubmitError] = useState<string>();
  const enrollment = useEnrollment(enrollmentId);
  const available = (containers.data ?? []).filter((container) => container.status === "running");

  async function startEnrollment() {
    if (!selectedContainer) return;
    setSubmitError(undefined);
    try {
      const result = await createEnrollment(Number(selectedContainer));
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
        {!containers.isLoading && available.length === 0 ? <p className="muted">Keine laufenden, noch auswählbaren LXCs entdeckt.</p> : null}
        {submitError ? <p className="error-state" role="alert">{submitError}</p> : null}
        <button className="primary-button" type="button" disabled={!selectedContainer} onClick={() => void startEnrollment()}>Enrollment starten</button>
      </> : <>
        <div className="section-heading"><div><h2>Enrollment-Fortschritt</h2><p className="muted">Container <Link to={`/containers/${selectedContainer}`}>VMID {selectedContainer}</Link></p></div><span className={state === "connected" ? "success-state" : state === "failed" ? "error-state" : "muted"}>{state ?? "wird geladen…"}</span></div>
        <ol className="enrollment-steps">{steps.map(([step, label]) => <li key={step} className={state === step ? "current" : steps.findIndex(([value]) => value === (state ?? "requested")) >= steps.findIndex(([value]) => value === step) ? "complete" : ""}><span aria-hidden="true">{state === step ? "…" : "✓"}</span>{label}</li>)}</ol>
        {enrollment.isLoading ? <p className="muted">Status wird geladen…</p> : null}
        {enrollment.error ? <p className="error-state" role="alert">{enrollment.error.message}</p> : null}
        {state === "failed" ? <><p className="error-state" role="alert">{enrollment.data?.failure_reason ?? "Der Agent konnte nicht registriert werden."}</p><button className="primary-button" type="button" onClick={retry}>Erneut versuchen</button></> : null}
        {state === "connected" ? <p className="success-state">Agent verbunden. <Link to={`/containers/${selectedContainer}`}>Health und Metriken öffnen</Link></p> : null}
      </>}
    </section>
  </>;
}
