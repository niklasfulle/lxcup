import { cn, ui } from "../ui";
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
    <section className={cn(ui.panel, ui.lifecyclePanel)} aria-label={`Onboarding-Status für ${target.name}`}>
      <div className={ui.sectionHeading}>
        <div>
          <p className={ui.eyebrow}>Onboarding-Fortschritt</p>
          <h2>{target.name}</h2>
          <p className={ui.muted}>{target.kind.toUpperCase()} · {target.address}{target.agent_version ? ` · Agent v${target.agent_version}` : " · Noch keine Agent-Version gemeldet"}</p>
        </div>
        <span className={cn(ui.statusBadge, statusClass === "success" ? ui.statusSuccess : statusClass === "neutral" ? ui.statusNeutral : ui.statusPending)}>{statusLabel}</span>
      </div>

      <ol className={ui.lifecycleSteps}>
        {steps.map((step, index) => {
          const status = stepStatus(target, step.key);
          return (
            <li className={cn(ui.lifecycleStep, status === "complete" && ui.lifecycleComplete, status === "current" && ui.lifecycleCurrent)} key={step.key}>
              <span className={ui.lifecycleMarker} aria-hidden="true">
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
    return <div className={cn(ui.callout, ui.calloutSuccess)}><strong>Der LXC ist bereit.</strong><p>Der Agent{target.agent_version ? ` v${target.agent_version}` : ""} ist verbunden. Healthchecks und Workflows können jetzt ausgeführt werden.</p><Link className={ui.textLink} to="/workflows">Workflows öffnen →</Link></div>;
  }
  if (isDisabled) {
    return <div className={ui.callout}><strong>Das Ziel ist deaktiviert.</strong><p>Es werden keine Agenten- oder Workflow-Aktionen ausgeführt.</p></div>;
  }
  return <div className={cn(ui.callout, ui.calloutInfo)}><strong>Warum steht der Status auf „pending“?</strong><p>Das Ziel ist angelegt. Es wird erst als verbunden markiert, wenn der Agent bereitgestellt wurde und den ersten Heartbeat sendet.</p><div className={ui.actionRow}><Link className={ui.primaryButton} to={`/workflows?target=${encodeURIComponent(target.id)}`}>Agent-Workflow öffnen</Link>{target.kind === "lxc" && <Link className={ui.textLink} to="/enrollments/new">LXC-Auswahl öffnen</Link>}</div></div>;
}
