import { cn } from "../classnames";
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
    <section className={cn("mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]", "mb-3")} aria-label={`Onboarding-Status für ${target.name}`}>
      <div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start">
        <div>
          <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Onboarding-Fortschritt</p>
          <h2>{target.name}</h2>
          <p className="text-[var(--muted)]">{target.kind.toUpperCase()} · {target.address}{target.agent_version ? ` · Agent v${target.agent_version}` : " · Noch keine Agent-Version gemeldet"}</p>
        </div>
        <span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", statusBadgeClass(statusClass))}>{statusLabel}</span>
      </div>

      <ol className="my-4 grid list-none grid-cols-1 gap-3 p-0 md:grid-cols-3">
        {steps.map((step, index) => {
          const status = stepStatus(target, step.key);
          return (
            <li className={cn("flex items-start gap-2 border-t-2 border-[var(--line)] pt-2 text-[var(--muted)]", status === "complete" && "border-[#8bd4b2] text-[var(--ink)]", status === "current" && "border-lxcup-primary text-[var(--ink)]")} key={step.key}>
              <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full border border-[var(--line)] bg-[var(--panel)] text-xs font-bold text-[var(--muted)]" aria-hidden="true">
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

function statusBadgeClass(status: string) {
  if (status === "success") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (status === "neutral") return "bg-[var(--paper-muted)] text-[var(--muted)]";
  return "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function stepMarker(status: ReturnType<typeof stepStatus>, index: number) {
  if (status === "complete") return "✓";
  if (status === "blocked") return "—";
  if (status === "current") return "•";
  return index + 1;
}

function lifecycleCallout(target: TargetDto, isManaged: boolean, isDisabled: boolean) {
  if (isManaged) {
    return <div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#b7e7d0] bg-[var(--success-soft)]")}><strong>Der LXC ist bereit.</strong><p>Der Agent{target.agent_version ? ` v${target.agent_version}` : ""} ist verbunden. Healthchecks und Workflows können jetzt ausgeführt werden.</p><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/workflows">Workflows öffnen →</Link></div>;
  }
  if (isDisabled) {
    return <div className="border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]"><strong>Das Ziel ist deaktiviert.</strong><p>Es werden keine Agenten- oder Workflow-Aktionen ausgeführt.</p></div>;
  }
  return <div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}><strong>Warum steht der Status auf „pending“?</strong><p>Das Ziel ist angelegt. Es wird erst als verbunden markiert, wenn der Agent bereitgestellt wurde und den ersten Heartbeat sendet.</p><div className="flex flex-wrap items-center gap-2"><Link className="inline-flex justify-self-start items-center justify-center border border-lxcup-primary bg-lxcup-primary px-2.5 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" to={`/workflows?target=${encodeURIComponent(target.id)}`}>Agent-Workflow öffnen</Link>{target.kind === "lxc" && <Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/enrollments/new">LXC-Auswahl öffnen</Link>}</div></div>;
}
