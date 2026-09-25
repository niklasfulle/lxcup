import { cn } from "../classnames";
import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { createAnsibleJob, createEnrollment } from "../api";
import { useAnsibleJob, useContainers, useEnrollment, useTargets } from "../queries";

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
  const [searchParams] = useSearchParams();
  const [selectedContainer, setSelectedContainer] = useState("");
  const [selectedTarget, setSelectedTarget] = useState(() => searchParams.get("target") ?? "");
  const [enrollmentId, setEnrollmentId] = useState<string>();
  const [submitError, setSubmitError] = useState<string>();
  const [startOnboarding, setStartOnboarding] = useState(true);
  const [healthJobId, setHealthJobId] = useState<string>();
  const [packageJobId, setPackageJobId] = useState<string>();
  const healthStartedFor = useRef<string | undefined>(undefined);
  const packageStartedFor = useRef<string | undefined>(undefined);
  const enrollment = useEnrollment(enrollmentId);
  const healthJob = useAnsibleJob(healthJobId);
  const available = (containers.data ?? []).filter((container) => container.status === "running");

  async function startEnrollment() {
    if (selectedContainer === "" || (startOnboarding && selectedTarget === "")) return;
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

  const packageInventory = useMutation({
    mutationFn: () => createAnsibleJob({
      operation: "collect_package_inventory",
      target_id: selectedTarget || undefined,
      container_id: Number(selectedContainer),
      mode: "check",
      parameters: { operation: "collect_package_inventory" },
      idempotency_key: `enrollment-packages-${enrollmentId}`,
      confirmed: true,
    }),
    onSuccess: (job) => setPackageJobId(job.id),
  });

  useEffect(() => {
    if (startOnboarding === false || state !== "connected" || enrollmentId === undefined || healthStartedFor.current === enrollmentId) return;
    healthStartedFor.current = enrollmentId;
    health.mutate();
  }, [enrollmentId, health, startOnboarding, state]);

  useEffect(() => {
    if (startOnboarding === false || state !== "connected" || !enrollmentId || !healthJobId || healthJob.data?.status !== "succeeded" || packageStartedFor.current === healthJobId) return;
    packageStartedFor.current = healthJobId;
    packageInventory.mutate();
  }, [enrollmentId, healthJob.data?.status, healthJobId, packageInventory, startOnboarding, state]);

  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">LXC-Container</p><h1>LXC aufnehmen</h1><p className="text-[var(--muted)]">Verknüpfe einen entdeckten LXC mit seinem Zugangsprofil und rolle den Agenten kontrolliert aus.</p></div><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to="/targets/lxc">LXC-Zugang anlegen</Link></header>
    <section className={cn("mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]", "grid max-w-4xl gap-4")}>
      {enrollmentId ? <EnrollmentProgress enrollment={enrollment} selectedContainer={selectedContainer} state={state} startOnboarding={startOnboarding} health={health} healthJobId={healthJobId} healthStatus={healthJob.data?.status} packageInventory={packageInventory} packageJobId={packageJobId} onRetry={retry} /> : <EnrollmentStartForm available={available} targets={targets.data ?? []} selectedContainer={selectedContainer} selectedTarget={selectedTarget} startOnboarding={startOnboarding} submitError={submitError} containersLoading={containers.isLoading} onContainerChange={setSelectedContainer} onTargetChange={setSelectedTarget} onStartOnboardingChange={setStartOnboarding} onStart={startEnrollment} />}
    </section>
  </>;
}

function EnrollmentStartForm({ available, targets, selectedContainer, selectedTarget, startOnboarding, submitError, containersLoading, onContainerChange, onTargetChange, onStartOnboardingChange, onStart }: Readonly<{ available: Array<{ id: number; name: string }>; targets: Array<{ id: string; name: string; address: string; state: string; kind: string }>; selectedContainer: string; selectedTarget: string; startOnboarding: boolean; submitError?: string; containersLoading: boolean; onContainerChange: (value: string) => void; onTargetChange: (value: string) => void; onStartOnboardingChange: (value: boolean) => void; onStart: () => Promise<void> }>) {
  const eligibleTargets = targets.filter((target) => target.kind === "lxc" && target.state !== "disabled");
  return <><label className="grid max-w-lg gap-1 text-xs font-semibold text-[var(--muted)]" htmlFor="enrollment-container"><span>1. LXC aus dem Inventar auswählen</span><select id="enrollment-container" value={selectedContainer} onChange={(event) => onContainerChange(event.target.value)}><option value="">Bitte auswählen…</option>{available.map((container) => <option key={container.id} value={container.id}>{container.name} · VMID {container.id}</option>)}</select></label>{startOnboarding && <label className="grid max-w-lg gap-1 text-xs font-semibold text-[var(--muted)]" htmlFor="enrollment-target"><span>2. LXC-Zugangsprofil auswählen</span><select id="enrollment-target" value={selectedTarget} onChange={(event) => onTargetChange(event.target.value)}><option value="">Bitte Ziel auswählen…</option>{eligibleTargets.map((target) => <option key={target.id} value={target.id}>{target.name} · {target.address}</option>)}</select><span className="text-[var(--muted)]">Das Zugangsprofil liefert SSH-Zugang, Host-Key und Agent-Token für diesen LXC.</span></label>}<label className="flex items-center gap-2 text-sm font-medium"><input type="checkbox" checked={startOnboarding} onChange={(event) => onStartOnboardingChange(event.target.checked)} /><span>3. Agent installieren und nach erfolgreicher Verbindung einen Healthcheck ausführen.</span></label>{!containersLoading && available.length === 0 && <p className="text-[var(--muted)]">Keine laufenden, noch auswählbaren LXCs entdeckt.</p>}{startOnboarding && eligibleTargets.length === 0 ? <p className="text-[var(--muted)]">Noch kein LXC-Zugangsprofil vorhanden. <Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/targets/lxc">Jetzt anlegen →</Link></p> : null}{submitError && <p className="font-semibold text-[var(--error)]" role="alert">{submitError}</p>}<button className="inline-flex justify-self-start items-center justify-center border border-lxcup-primary bg-lxcup-primary px-2.5 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={!selectedContainer || (startOnboarding && !selectedTarget)} onClick={() => void onStart()}>LXC aufnehmen</button></>;
}

type HealthMutationView = Readonly<{ isPending: boolean; error: unknown }>;

function EnrollmentProgress({ enrollment, selectedContainer, state, startOnboarding, health, healthJobId, healthStatus, packageInventory, packageJobId, onRetry }: Readonly<{ enrollment: ReturnType<typeof useEnrollment>; selectedContainer: string; state: string | undefined; startOnboarding: boolean; health: HealthMutationView; healthJobId?: string; healthStatus?: string; packageInventory: HealthMutationView; packageJobId?: string; onRetry: () => void }>) {
  return <><div className="flex items-center justify-between gap-3 max-[720px]:flex-col max-[720px]:items-start"><div><h2>Enrollment-Fortschritt</h2><p className="text-[var(--muted)]">Container <Link to={`/containers/${selectedContainer}`}>VMID {selectedContainer}</Link></p></div><span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", enrollmentBadgeClass(state))}>{enrollmentStateLabel(state)}</span></div><ol className="m-0 grid list-none gap-2 p-0">{steps.map(([step, label], index) => <li key={step} className={cn("flex items-center gap-2 text-sm text-[var(--muted)]", stepClass(state, step, index))}><span className="grid h-6 w-6 place-items-center rounded-full border border-[var(--line)] text-xs" aria-hidden="true">{state === step ? "…" : "✓"}</span>{label}</li>)}</ol>{state && <div className={cn("border border-[var(--line)] bg-[var(--paper-muted)] p-3 text-[var(--ink)]", "border-[#bad0fa] bg-[var(--primary-soft)]")}><strong>Aktueller Schritt</strong><p>{stateDescriptions[state] ?? "Der Status wird vom Controller aktualisiert."}</p></div>}{enrollment.isLoading && <p className="text-[var(--muted)]">Status wird geladen…</p>}{enrollment.error && <p className="font-semibold text-[var(--error)]" role="alert">{enrollment.error.message}</p>}{state === "failed" && <><p className="font-semibold text-[var(--error)]" role="alert">{enrollment.data?.failure_reason ?? "Der Agent konnte nicht registriert werden."}</p><button className="inline-flex justify-self-start items-center justify-center border border-lxcup-primary bg-lxcup-primary px-2.5 py-1.5 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onRetry}>Erneut versuchen</button></>}{state === "connected" && <div className="border border-[#b7e7d0] bg-[var(--success-soft)] px-2 py-1.5 font-medium text-[var(--success)]">Agent verbunden. {connectedAction(startOnboarding, health, healthJobId, selectedContainer)}{health.error instanceof Error && <p className="font-semibold text-[var(--error)]">{health.error.message}</p>}{showPackageInventoryStep(startOnboarding, healthStatus) && <p>Paketinventar: {packageInventoryStatus(packageInventory.isPending, packageJobId)}</p>}{packageInventory.error instanceof Error && <p className="font-semibold text-[var(--error)]">{packageInventory.error.message}</p>}</div>}</>;
}

function enrollmentBadgeClass(state: string | undefined) {
  const status = enrollmentStatusClass(state);
  if (status === "success") return "bg-[var(--success-soft)] text-[var(--success)]";
  if (status === "neutral") return "bg-[var(--paper-muted)] text-[var(--muted)]";
  return "bg-[var(--warning-soft)] text-[var(--warning)]";
}

function showPackageInventoryStep(startOnboarding: boolean, healthStatus: string | undefined) {
  return startOnboarding && healthStatus === "succeeded";
}

function packageInventoryStatus(pending: boolean, jobId: string | undefined) {
  if (pending) return "wird erstellt…";
  if (jobId) return <Link to={`/workflows/${jobId}`}>Protokoll öffnen</Link>;
  return "wird vorbereitet.";
}

function enrollmentStateLabel(state: string | undefined) {
  if (state === undefined) return "Wird geladen…";
  return stateLabels[state] ?? state;
}

function enrollmentStatusClass(state: string | undefined) {
  if (state === "connected") return "success";
  if (state === "failed") return "neutral";
  return "pending";
}

function stepClass(state: string | undefined, step: string, index: number) {
  if (state === step) return "border-lxcup-primary font-semibold text-[var(--ink)]";
  const current = steps.findIndex(([value]) => value === (state ?? "requested"));
  return current >= index ? "border-[#8bd4b2] text-[var(--ink)]" : "";
}

function connectedAction(startOnboarding: boolean, health: HealthMutationView, healthJobId: string | undefined, selectedContainer: string) {
  if (startOnboarding === false) return <Link to={`/containers/${selectedContainer}`}>Health und Metriken öffnen</Link>;
  if (health.isPending) return "Der Healthcheck wird gestartet…";
  if (healthJobId) return <Link to={`/workflows/${healthJobId}`}>Healthcheck-Protokoll öffnen</Link>;
  return "Der Healthcheck wird vorbereitet.";
}
