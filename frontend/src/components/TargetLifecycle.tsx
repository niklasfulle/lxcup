import { Link } from "react-router-dom";
import type { TargetDto } from "../api";

const steps = [
  { key: "registered", label: "Ziel registriert", description: "Die Verbindungsdaten wurden sicher gespeichert." },
  { key: "agent", label: "Agent bereitstellen", description: "Der freigegebene Agent-Workflow muss auf dem Ziel laufen." },
  { key: "heartbeat", label: "Erster Heartbeat", description: "Der Agent meldet sich beim Controller und bestätigt die Verbindung." },
] as const;

type StepKey = (typeof steps)[number]["key"];

function stepStatus(target: TargetDto, step: StepKey): "complete" | "current" | "pending" | "blocked" {
  if (target.state === "disabled") return step === "registered" ? "complete" : "blocked";
  if (target.state === "managed") return "complete";
  if (step === "registered") return "complete";
  if (step === "agent") return "current";
  return "pending";
}

export function TargetLifecycle({ target }: Readonly<{ target: TargetDto }>) {
  const isManaged = target.state === "managed";
  const isDisabled = target.state === "disabled";
  const statusLabel = lifecycleStatusLabel(target.state);
  const statusClass = lifecycleStatusClass(target.state);

  return (
    <section className="panel lifecycle-panel" aria-label={`Onboarding-Status für ${target.name}`}>
      <div className="section-heading lifecycle-heading">
        <div>
          <p className="eyebrow">Onboarding-Fortschritt</p>
          <h2>{target.name}</h2>
          <p className="muted">{target.kind.toUpperCase()} · {target.address}</p>
        </div>
        <span className={`status-badge ${statusClass}`}>{statusLabel}</span>
      </div>

      <ol className="lifecycle-steps">
        {steps.map((step, index) => {
          const status = stepStatus(target, step.key);
          return (
            <li className={`lifecycle-step ${status}`} key={step.key}>
              <span className="lifecycle-marker" aria-hidden="true">
                {stepMarker(status, index)}
              </span>
              <div>
                <strong>{step.label}</strong>
                <p>{step.description}</p>
              </div>
            </li>
          );
        })}
      </ol>

      {lifecycleCallout(target, isManaged, isDisabled)}
    </section>
  );
}

function lifecycleStatusLabel(state: TargetDto["state"]) {
  if (state === "managed") return "Verbunden";
  if (state === "disabled") return "Deaktiviert";
  return "Wartet auf Agent";
}

function lifecycleStatusClass(state: TargetDto["state"]) {
  if (state === "managed") return "success";
  if (state === "disabled") return "neutral";
  return "pending";
}

function stepMarker(status: ReturnType<typeof stepStatus>, index: number) {
  if (status === "complete") return "✓";
  if (status === "blocked") return "—";
  if (status === "current") return "•";
  return index + 1;
}

function lifecycleCallout(target: TargetDto, isManaged: boolean, isDisabled: boolean) {
  if (isManaged) {
    return <div className="callout success"><strong>Der LXC ist bereit.</strong><p>Der Agent ist verbunden. Healthchecks und Workflows können jetzt ausgeführt werden.</p><Link className="text-link" to="/workflows">Workflows öffnen →</Link></div>;
  }
  if (isDisabled) {
    return <div className="callout"><strong>Das Ziel ist deaktiviert.</strong><p>Es werden keine Agenten- oder Workflow-Aktionen ausgeführt.</p></div>;
  }
  return <div className="callout info"><strong>Warum steht der Status auf „pending“?</strong><p>Das Ziel ist angelegt. Es wird erst als verbunden markiert, wenn der Agent bereitgestellt wurde und den ersten Heartbeat sendet.</p><div className="callout-actions"><Link className="primary-button" to={`/workflows?target=${encodeURIComponent(target.id)}`}>Agent-Workflow öffnen</Link>{target.kind === "lxc" && <Link className="text-link" to="/enrollments/new">LXC-Auswahl öffnen</Link>}</div></div>;
}
