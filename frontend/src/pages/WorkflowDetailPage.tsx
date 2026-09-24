import { cn, ui } from "../ui";
import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import type { AnsibleJobEvent } from "../api";
import { useAnsibleJob, useAnsibleJobEvents } from "../queries";

const terminalStates = new Set(["succeeded", "failed", "aborted"]);

export function WorkflowDetailPage() {
  const { jobId } = useParams();
  const job = useAnsibleJob(jobId);
  const active = !terminalStates.has(job.data?.status ?? "queued");
  const events = useAnsibleJobEvents(jobId, true);

  if (job.isLoading) return <section className={ui.panel}><p className={ui.muted}>Lade Workflow…</p></section>;
  if (job.error !== null || job.data === undefined) return <section className={ui.panel}><h1>Workflow nicht gefunden</h1><p className={ui.errorState}>{job.error?.message ?? "Der Job ist nicht mehr verfügbar."}</p><Link className={ui.textLink} to="/workflows">Zur Workflow-Übersicht</Link></section>;

  const currentJob = job.data;
  const latestEvent = events.data?.at(-1);
  const guidance = jobGuidance(currentJob.status, latestEvent);
  const rawLog = events.data?.map((event) => `[${new Date(event.created_at).toISOString()}] #${event.sequence} ${eventTitle(event)}\n${eventDetail(event)}`).join("\n\n") ?? "";
  return <>
    <header className={ui.pageHeader}><div><p className={ui.eyebrow}>Workflow-Protokoll</p><h1>{currentJob.operation.replaceAll("_", " ")}</h1><p className={ui.muted}>Job {currentJob.id}</p></div><Link className={ui.button} to="/workflows">← Alle Workflows</Link></header>
    <div className={ui.panelGrid}><section className={ui.panel}><h2>Ausführung</h2><dl className={ui.detailList}><dt>Status</dt><dd><span className={cn(ui.statusBadge, statusClass(currentJob.status) === "success" ? ui.statusSuccess : statusClass(currentJob.status) === "neutral" ? ui.statusNeutral : ui.statusPending)}>{currentJob.status}</span></dd><dt>Modus</dt><dd>{currentJob.mode}</dd><dt>Playbook</dt><dd>{currentJob.playbook} · v{currentJob.playbook_version}</dd></dl></section><section className={ui.panel}><h2>Zeiten</h2><dl className={ui.detailList}><dt>Erstellt</dt><dd>{new Date(currentJob.created_at).toLocaleString()}</dd><dt>Letzte Änderung</dt><dd>{new Date(currentJob.updated_at).toLocaleString()}</dd><dt>Protokoll</dt><dd>{active ? "wird automatisch aktualisiert" : "abgeschlossen"}</dd></dl></section></div>
    <section className={cn(ui.callout, guidance.level === "success" ? ui.calloutSuccess : guidance.level === "danger" ? ui.calloutDanger : ui.calloutInfo)} aria-live="polite"><strong>{guidance.title}</strong><p>{guidance.detail}</p></section>
    <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Ausführungsprotokoll</h2><p className={ui.muted}>Zeitlich sortierte, audit-sichere Schritte und Fehlerhinweise dieses Jobs.</p></div><span className={ui.muted}>{events.data?.length ?? 0} Einträge</span></div>{workflowLogContent(events, rawLog)}</section>
  </>;
}

function CopyLogButton({ log }: Readonly<{ log: string }>) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    await navigator.clipboard.writeText(log);
    setCopied(true);
    globalThis.setTimeout(() => setCopied(false), 2_000);
  };
  return <button className={cn(ui.button, "px-3 py-1")} type="button" onClick={copy}>{copied ? "Log kopiert" : "Log kopieren"}</button>;
}

function statusClass(status: string) {
  if (status === "succeeded") return "success";
  if (status === "failed" || status === "aborted") return "neutral";
  return "pending";
}

function workflowLogContent(events: ReturnType<typeof useAnsibleJobEvents>, rawLog: string) {
  if (events.isLoading) return <p className={ui.muted}>Lade Protokoll…</p>;
  if (events.error) return <p className={ui.errorState} role="alert">{events.error.message}</p>;
  if (events.data === undefined || events.data.length === 0) return <p className={ui.emptyState}>Noch keine Worker-Meldung. Prüfe, ob ein Ansible-Worker läuft und das Ziel erreichbar ist.</p>;
  return <><ol className={ui.jobLog}>{events.data.map((event) => <li key={event.sequence}><span className={ui.jobLogSequence}>{event.sequence}</span><div><strong>{eventTitle(event)}</strong><p>{eventDetail(event)}</p></div><time className={ui.muted}>{new Date(event.created_at).toLocaleString()}</time></li>)}</ol><div className={ui.jobLogHeading}><h3>Vollständiger technischer Log</h3><CopyLogButton log={rawLog} /></div><pre className={ui.rawLog}>{rawLog}</pre></>;
}

function eventTitle(event: AnsibleJobEvent) {
  const kind = event.event.kind;
  if (kind === "queued") return "Job eingereiht";
  if (kind === "status_changed") return `Status: ${event.event.status ?? "aktualisiert"}`;
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
  if (status === "failed") return { level: "danger", title: "Workflow fehlgeschlagen", detail: event?.event.code ? `Der Worker meldet den Fehlercode „${event.event.code}“. Die betroffene Aufgabe steht im Protokoll.` : "Der Worker hat einen Fehler gemeldet. Prüfe den letzten Protokolleintrag und die Zielverbindung." };
  if (status === "reconcile_required") return { level: "danger", title: "Abgleich erforderlich", detail: "Die Ausführung wurde unterbrochen oder ihr Ergebnis ist unklar. Starte einen Reconcile-Workflow, bevor du weitere Änderungen ausführst." };
  if (status === "succeeded") return { level: "success", title: "Workflow abgeschlossen", detail: "Alle vom Worker gemeldeten Schritte sind im Protokoll dokumentiert." };
  return { level: "info", title: "Workflow wird ausgeführt", detail: "Das Protokoll aktualisiert sich automatisch, sobald der Worker einen Schritt meldet." };
}
