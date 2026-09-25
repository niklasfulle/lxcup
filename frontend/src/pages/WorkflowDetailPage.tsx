import { cn } from "../classnames";
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "react-router-dom";
import { reconcileAnsibleJob, retryAnsibleJob, type AnsibleJobEvent, type AnsibleJobDto, type ContainerDto, type TargetDto } from "../api";
import { queryKeys, useAnsibleJob, useAnsibleJobEvents, useContainers, useTargets } from "../queries";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";

const terminalStates = new Set(["succeeded", "failed", "aborted"]);

export function WorkflowDetailPage() {
  const { jobId } = useParams();
  const queryClient = useQueryClient();
  const job = useAnsibleJob(jobId);
  const targets = useTargets();
  const containers = useContainers();
  const active = !terminalStates.has(job.data?.status ?? "queued");
  const events = useAnsibleJobEvents(jobId, true);
  const retry = useMutation({
    mutationFn: () => retryAnsibleJob(jobId!, job.data?.mode === "apply"),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJob(jobId!) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobEvents(jobId!) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }),
      ]);
    },
  });
  const reconcile = useMutation({
    mutationFn: () => reconcileAnsibleJob(jobId!),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJob(jobId!) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.ansibleJobs }),
      ]);
    },
  });

  if (job.isLoading) return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><p className="text-[var(--muted)]">Lade Workflow…</p></section>;
  if (job.error !== null || job.data === undefined) return <section className="mb-3 border border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)]"><h1>Workflow nicht gefunden</h1><p className="font-semibold text-[var(--error)]">{job.error?.message ?? "Der Job ist nicht mehr verfügbar."}</p><Link className="mt-3 inline-block text-xs font-semibold text-lxcup-primary hover:underline" to="/workflows">Zur Workflow-Übersicht</Link></section>;

  const currentJob = job.data;
  const resource = resolveJobResource(currentJob, targets.data ?? [], containers.data ?? []);
  const latestEvent = events.data?.at(-1);
  const guidance = jobGuidance(currentJob.status, latestEvent);
  const rawLog = events.data?.map((event) => `[${new Date(event.created_at).toISOString()}] #${event.sequence} ${eventTitle(event)}\n${eventDetail(event)}`).join("\n\n") ?? "";
  const retryAllowed = currentJob.status === "failed" && !(currentJob.operation === "update_packages" && currentJob.mode === "apply");
  const retryJob = () => {
    if (currentJob.mode === "apply" && !globalThis.confirm("Diesen fehlgeschlagenen Änderungslauf erneut ausführen?")) return;
    retry.mutate();
  };
  return <>
    <header className="mb-3 flex items-end justify-between gap-4 border-b border-[var(--line)] pb-3 max-[720px]:flex-col max-[720px]:items-start"><div className="min-w-0"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Workflow-Protokoll</p><h1>{workflowOperationLabel(currentJob.operation)}</h1><p className="truncate text-[var(--muted)]" title={currentJob.id}>Job {currentJob.id} <span aria-hidden="true">·</span> {resource.name}</p></div><div className="flex shrink-0 flex-wrap items-center gap-2"><WorkflowReconcileAction status={currentJob.status} mode={currentJob.mode} pending={reconcile.isPending} createdJobId={reconcile.data?.id} onReconcile={() => reconcile.mutate()} /><WorkflowRetryAction status={currentJob.status} allowed={retryAllowed} pending={retry.isPending} onRetry={retryJob} /><Link className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" to="/workflows">← Alle Workflows</Link></div></header>
    {retry.error ? <p className="border border-[var(--error)] bg-[var(--error-soft)] px-3 py-2 font-semibold text-[var(--error)]" role="alert">{retry.error.message}</p> : null}
    {reconcile.error ? <p className="border border-[var(--error)] bg-[var(--error-soft)] px-3 py-2 font-semibold text-[var(--error)]" role="alert">{reconcile.error.message}</p> : null}
    <div className="mb-3 grid grid-cols-1 gap-3 lg:grid-cols-2">
      <section className="border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><h2 className="mb-3">Ausführung</h2><dl className="m-0 grid grid-cols-[minmax(7rem,0.7fr)_minmax(0,1.3fr)] gap-x-3 gap-y-3 text-sm"><dt className="text-[var(--muted)]">Status</dt><dd className="m-0"><span className={cn("inline-flex items-center px-2 py-0.5 text-xs font-bold", jobStatusBadgeClass(currentJob.status))}>{jobStatusLabel(currentJob.status)}</span></dd><dt className="text-[var(--muted)]">Ressource</dt><dd className="m-0 min-w-0"><strong>{resource.name}</strong>{resource.detail ? <small className="block text-[var(--muted)]">{resource.detail}</small> : null}</dd><dt className="text-[var(--muted)]">Modus</dt><dd className="m-0">{workflowModeLabel(currentJob.mode)}</dd><dt className="text-[var(--muted)]">Playbook</dt><dd className="m-0 min-w-0 break-words">{currentJob.playbook} · v{currentJob.playbook_version}</dd>{currentJob.approved_plan_job_id ? <><dt className="text-[var(--muted)]">Freigegebener Paketplan</dt><dd className="m-0 min-w-0 break-all"><Link className="font-semibold text-lxcup-primary hover:underline" to={`/workflows/${currentJob.approved_plan_job_id}`}>{currentJob.approved_plan_job_id}</Link></dd></> : null}{currentJob.reconciles_job_id ? <><dt className="text-[var(--muted)]">Abgleich für Job</dt><dd className="m-0 min-w-0 break-all"><Link className="font-semibold text-lxcup-primary hover:underline" to={`/workflows/${currentJob.reconciles_job_id}`}>{currentJob.reconciles_job_id}</Link></dd></> : null}</dl></section>
      <section className="border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]"><h2 className="mb-3">Zeiten</h2><dl className="m-0 grid grid-cols-[minmax(7rem,0.7fr)_minmax(0,1.3fr)] gap-x-3 gap-y-3 text-sm"><dt className="text-[var(--muted)]">Erstellt</dt><dd className="m-0">{new Date(currentJob.created_at).toLocaleString()}</dd><dt className="text-[var(--muted)]">Letzte Änderung</dt><dd className="m-0">{new Date(currentJob.updated_at).toLocaleString()}</dd><dt className="text-[var(--muted)]">Protokoll</dt><dd className="m-0">{active ? "wird automatisch aktualisiert" : "abgeschlossen"}</dd></dl></section>
    </div>
    <section className={cn("mb-3 border p-4 text-[var(--ink)]", guidanceClass(guidance.level))} aria-live="polite"><strong>{guidance.title}</strong><p className="mb-0 mt-1 text-sm">{guidance.detail}</p></section>
    <section className="mb-3 overflow-hidden border border-[var(--line)] bg-[var(--panel)] text-[var(--ink)]">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] px-4 py-3"><div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Audit-Trail</p><h2 className="m-0">Ausführungsprotokoll</h2><p className="mt-1 text-sm text-[var(--muted)]">Zeitlich sortierte Schritte und Rückmeldungen des Workers.</p></div><span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 py-1 text-xs font-semibold text-[var(--muted)]">{events.data?.length ?? 0} Einträge</span></div>
      {workflowLogContent(events, rawLog, currentJob.mode === "plan")}
    </section>
  </>;
}

function WorkflowRetryAction({ status, allowed, pending, onRetry }: Readonly<{ status: string; allowed: boolean; pending: boolean; onRetry: () => void }>) {
  if (status !== "failed") return null;
  if (!allowed) return <span className="text-[var(--muted)]" title="Paket-Apply-Jobs müssen neu geplant und erneut bestätigt werden.">Neuer Update-Plan erforderlich</span>;
  return <button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={pending} onClick={onRetry}>{pending ? "Wird erneut eingereiht…" : "Fehlgeschlagenen Job erneut versuchen"}</button>;
}

function WorkflowReconcileAction({ status, mode, pending, createdJobId, onReconcile }: Readonly<{ status: string; mode: string; pending: boolean; createdJobId: string | undefined; onReconcile: () => void }>) {
  if (status !== "reconcile_required" || mode !== "apply") return null;
  const buttonLabel = reconcileButtonLabel(pending, createdJobId);
  return <div className="flex flex-wrap items-center gap-2">
    <button className="inline-flex items-center justify-center border border-[var(--error)] bg-[var(--error-soft)] px-2 py-1.5 text-xs font-semibold text-[var(--error)] hover:brightness-95 disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={pending || Boolean(createdJobId)} onClick={onReconcile}>{buttonLabel}</button>
    {createdJobId ? <Link className="text-xs font-semibold text-lxcup-primary hover:underline" to={`/workflows/${createdJobId}`}>Abgleich öffnen</Link> : null}
  </div>;
}

function reconcileButtonLabel(pending: boolean, createdJobId: string | undefined) {
  if (pending) return "Abgleich wird eingereiht…";
  if (createdJobId) return "Abgleich eingereiht";
  return "Ist-Zustand abgleichen";
}

function guidanceClass(level: string) {
  if (level === "success") return "border-[#b7e7d0] bg-[var(--success-soft)]";
  if (level === "danger") return "border-red-200 bg-[var(--error-soft)]";
  return "border-[#bad0fa] bg-[var(--primary-soft)]";
}

function CopyLogButton({ log }: Readonly<{ log: string }>) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(log);
    setCopied(true);
    globalThis.setTimeout(() => setCopied(false), 2_000);
  };
  return <button className={cn("inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50", "px-3 py-1")} type="button" onClick={copy}>{copied ? "Log kopiert" : "Log kopieren"}</button>;
}

function workflowLogContent(events: ReturnType<typeof useAnsibleJobEvents>, rawLog: string, isPlan: boolean) {
  if (events.isLoading) return <p className="text-[var(--muted)]">Lade Protokoll…</p>;
  if (events.error) return <p className="font-semibold text-[var(--error)]" role="alert">{events.error.message}</p>;
  if (events.data === undefined || events.data.length === 0) return <p className="m-4 grid min-h-24 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-3 text-sm text-[var(--muted)]">Noch keine Worker-Meldung. Prüfe, ob ein Ansible-Worker läuft und das Ziel erreichbar ist.</p>;
  const planOutput = isPlan ? events.data.filter((event) => event.event.kind === "worker_log" && event.event.source === "stdout").map((event) => event.event.kind === "worker_log" ? event.event.message : "").filter(Boolean).join("\n") : "";
  const planSummary = isPlan ? events.data.find((event) => event.event.kind === "worker_log" && event.event.source === "plan") : undefined;
  return <>
    {isPlan ? <PlanPreview summary={planSummary?.event.kind === "worker_log" ? planSummary.event.message ?? "Vorschau wird erstellt…" : "Vorschau wird erstellt…"} preview={planOutput} copyable={Boolean(planOutput || planSummary)} /> : null}
    <ol className="m-0 list-none divide-y divide-[var(--line)] p-0">
      {events.data.map((event) => <li className="grid grid-cols-[2rem_minmax(0,1fr)_auto] items-start gap-x-3 px-4 py-3 max-[600px]:grid-cols-[2rem_minmax(0,1fr)]" key={event.sequence}>
        <span className={cn("mt-0.5 grid h-7 w-7 place-items-center rounded-full text-xs font-bold ring-4 ring-[var(--panel)]", eventStepClass(event))}>{event.sequence}</span>
        <div className="min-w-0"><strong className="block text-sm leading-5">{eventTitle(event)}</strong>{hasEventSummary(event) ? <p className={cn("mb-0 mt-1 break-words text-sm text-[var(--muted)]", event.event.kind === "failed" && "font-medium text-[var(--error)]")}>{eventSummary(event)}</p> : null}</div>
        <time className="pt-1 text-right text-xs text-[var(--muted)] max-[600px]:col-start-2 max-[600px]:row-start-2 max-[600px]:pt-0" dateTime={event.created_at}>{new Date(event.created_at).toLocaleString()}</time>
      </li>)}
    </ol>
    <div className="border-t border-[var(--line)] bg-[var(--paper-muted)] p-4">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-3"><div><h3 className="m-0 text-sm font-semibold">Vollständiger technischer Log</h3><p className="m-0 mt-1 text-xs text-[var(--muted)]">Ungekürzte Controller- und Workerausgabe zum Kopieren und Debuggen.</p></div><CopyLogButton log={rawLog} /></div>
      <pre className="m-0 max-h-[32rem] overflow-auto whitespace-pre-wrap break-words border border-[var(--line)] bg-[#111b2d] p-4 font-mono text-xs leading-relaxed text-[var(--ink)]">{rawLog}</pre>
    </div>
  </>;
}

function PlanPreview({ summary, preview, copyable }: Readonly<{ summary: string; preview: string; copyable: boolean }>) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(preview || summary);
    setCopied(true);
    globalThis.setTimeout(() => setCopied(false), 2_000);
  };
  return <section className="border-b border-[var(--line)] bg-[var(--primary-soft)] p-4" aria-label="Plan-Vorschau">
    <div className="mb-2 flex flex-wrap items-start justify-between gap-3"><div><h3 className="m-0 text-sm font-semibold text-[var(--ink)]">Erwartete Änderungen</h3><p className="mb-0 mt-1 text-sm text-[var(--muted)]">{summary}</p></div><button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--paper-muted)] disabled:cursor-not-allowed disabled:opacity-50" type="button" disabled={!copyable} onClick={copy}>{copied ? "Vorschau kopiert" : "Vorschau kopieren"}</button></div>
    {preview ? <pre className="m-0 max-h-96 overflow-auto whitespace-pre-wrap break-words border border-[var(--line)] bg-[#111b2d] p-3 font-mono text-xs leading-relaxed text-[var(--ink)]">{preview}</pre> : <p className="m-0 border border-dashed border-[var(--line)] p-3 text-sm text-[var(--muted)]">Die Diff-Vorschau erscheint, sobald der Worker die Prüfung abgeschlossen hat.</p>}
  </section>;
}

function eventStepClass(event: AnsibleJobEvent) {
  if (event.event.kind === "failed" || event.event.kind === "reconcile_required") return "bg-[var(--error-soft)] text-[var(--error)]";
  if (event.event.kind === "task_finished" && event.event.changed) return "bg-[var(--success-soft)] text-[var(--success)]";
  return "bg-[var(--primary-soft)] text-lxcup-primary";
}

function hasEventSummary(event: AnsibleJobEvent) {
  return event.event.kind === "task_finished" || event.event.kind === "failed" || event.event.kind === "reconcile_required" || event.event.kind === "worker_log";
}

function eventSummary(event: AnsibleJobEvent) {
  if (event.event.kind === "worker_log") {
    const message = event.event.message ?? "(keine Ausgabe)";
    return message.length > 260 ? `${message.slice(0, 260)}…` : message;
  }
  return eventDetail(event);
}

function workflowOperationLabel(operation: string) {
  const labels: Record<string, string> = {
    deploy_agent: "Agent installieren",
    update_agent: "Agent aktualisieren",
    repair_agent: "Agent reparieren",
    update_packages: "Pakete aktualisieren",
    collect_package_inventory: "Paketinventar erfassen",
    health_check: "Healthcheck",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}

function workflowModeLabel(mode: string) {
  if (mode === "plan") return "Vorschau / Dry-Run";
  if (mode === "reconcile") return "Abgleich";
  return mode === "apply" ? "Apply" : "Check";
}

function resolveJobResource(job: AnsibleJobDto, targets: TargetDto[], containers: ContainerDto[]) {
  if ("container" in job.target) {
    const id = (job.target as { container: number }).container;
    const container = containers.find((item) => item.id === id);
    const operatingSystem = container?.operating_system;
    const detail = operatingSystem ? `LXC · ${operatingSystem}` : "LXC";
    return { name: container?.name ?? `Container ${id}`, detail };
  }
  if ("node" in job.target) return { name: `Node ${(job.target as { node: string }).node}`, detail: "Server-Node" };
  const id = (job.target as { target: string }).target;
  const target = targets.find((item) => item.id === id);
  return target ? { name: target.name, detail: `${target.kind} · ${target.address}` } : { name: `Ziel ${id.slice(0, 8)}`, detail: undefined };
}

function eventTitle(event: AnsibleJobEvent) {
  const kind = event.event.kind;
  if (kind === "queued") return "Job eingereiht";
  if (kind === "status_changed") return `Status: ${event.event.status ? jobStatusLabel(event.event.status) : "aktualisiert"}`;
  if (kind === "task_started") return `Task gestartet: ${event.event.task ?? "unbekannt"}`;
  if (kind === "task_finished") return `Task beendet: ${event.event.task ?? "unbekannt"}`;
  if (kind === "failed") return "Workflow fehlgeschlagen";
  if (kind === "worker_log") return `Worker · ${event.event.source ?? "stdout"}`;
  return "Abgleich erforderlich";
}

function eventDetail(event: AnsibleJobEvent) {
  if (event.event.kind === "task_finished") return event.event.changed ? "Die Aufgabe hat Änderungen vorgenommen." : "Die Aufgabe wurde ohne Änderungen beendet.";
  if (event.event.kind === "failed") return `Fehlercode: ${event.event.code ?? "unbekannt"}`;
  if (event.event.kind === "reconcile_required") return "Der Status muss durch einen kontrollierten Abgleich bestätigt werden.";
  if (event.event.kind === "worker_log") return event.event.message ?? "(keine Ausgabe)";
  return "Vom Workflow-Controller erfasst.";
}

function jobGuidance(status: string, event: AnsibleJobEvent | undefined) {
  if (status === "queued") return { level: "info", title: "Wartet auf den Ansible-Worker", detail: "Der Controller hat den Job angenommen, aber noch keine Worker-Ausführung erhalten. Prüfe Worker, Zielverbindung und Secrets." };
  if (status === "failed" && event?.event.code === "host_key_changed") return { level: "danger", title: "SSH-Host-Key stimmt nicht überein", detail: "Der gespeicherte Known-Hosts-Fingerprint passt nicht mehr. Verifiziere den neuen Fingerprint über einen vertrauenswürdigen Kanal, aktualisiere anschließend das Known-Hosts-Secret und starte den Job erneut. Deaktiviere die Host-Key-Prüfung nicht." };
  if (status === "failed") return { level: "danger", title: "Workflow fehlgeschlagen", detail: event?.event.code ? `Der Worker meldet den Fehlercode „${event.event.code}“. Die betroffene Aufgabe steht im Protokoll.` : "Der Worker hat einen Fehler gemeldet. Prüfe den letzten Protokolleintrag und die Zielverbindung." };
  if (status === "reconcile_required") return { level: "danger", title: "Abgleich erforderlich", detail: "Die Ausführung wurde unterbrochen oder ihr Ergebnis ist unklar. Starte einen Reconcile-Workflow, bevor du weitere Änderungen ausführst." };
  if (status === "succeeded") return { level: "success", title: "Workflow abgeschlossen", detail: "Alle vom Worker gemeldeten Schritte sind im Protokoll dokumentiert." };
  return { level: "info", title: "Workflow wird ausgeführt", detail: "Das Protokoll aktualisiert sich automatisch, sobald der Worker einen Schritt meldet." };
}
