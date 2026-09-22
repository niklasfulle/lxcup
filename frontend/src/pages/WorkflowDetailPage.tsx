import { Link, useParams } from "react-router-dom";
import type { AnsibleJobEvent } from "../api";
import { useAnsibleJob, useAnsibleJobEvents } from "../queries";

const terminalStates = new Set(["succeeded", "failed", "aborted"]);

export function WorkflowDetailPage() {
  const { jobId } = useParams();
  const job = useAnsibleJob(jobId);
  const active = !terminalStates.has(job.data?.status ?? "queued");
  const events = useAnsibleJobEvents(jobId, active);

  if (job.isLoading) return <section className="panel"><p className="muted">Lade Workflow…</p></section>;
  if (job.error || !job.data) return <section className="panel"><h1>Workflow nicht gefunden</h1><p className="error-state">{job.error?.message ?? "Der Job ist nicht mehr verfügbar."}</p><Link className="text-link" to="/workflows">Zur Workflow-Übersicht</Link></section>;

  const currentJob = job.data;
  const latestEvent = events.data?.at(-1);
  const guidance = jobGuidance(currentJob.status, latestEvent);
  return <>
    <header className="page-header"><div><p className="eyebrow">Workflow-Protokoll</p><h1>{currentJob.operation.replaceAll("_", " ")}</h1><p className="muted">Job {currentJob.id}</p></div><Link className="secondary-button" to="/workflows">← Alle Workflows</Link></header>
    <div className="panel-grid"><section className="panel"><h2>Ausführung</h2><dl className="detail-list"><dt>Status</dt><dd><span className={`status-badge ${currentJob.status === "succeeded" ? "success" : currentJob.status === "failed" || currentJob.status === "aborted" ? "neutral" : "pending"}`}>{currentJob.status}</span></dd><dt>Modus</dt><dd>{currentJob.mode}</dd><dt>Playbook</dt><dd>{currentJob.playbook} · v{currentJob.playbook_version}</dd></dl></section><section className="panel"><h2>Zeiten</h2><dl className="detail-list"><dt>Erstellt</dt><dd>{new Date(currentJob.created_at).toLocaleString()}</dd><dt>Letzte Änderung</dt><dd>{new Date(currentJob.updated_at).toLocaleString()}</dd><dt>Protokoll</dt><dd>{active ? "wird automatisch aktualisiert" : "abgeschlossen"}</dd></dl></section></div>
    <section className={`callout ${guidance.level}`} role={guidance.level === "danger" ? "alert" : "status"}><strong>{guidance.title}</strong><p>{guidance.detail}</p></section>
    <section className="panel"><div className="section-heading"><div><h2>Ausführungsprotokoll</h2><p className="muted">Zeitlich sortierte, audit-sichere Schritte und Fehlerhinweise dieses Jobs.</p></div><span className="muted">{events.data?.length ?? 0} Einträge</span></div>{events.isLoading ? <p className="muted">Lade Protokoll…</p> : events.error ? <p className="error-state" role="alert">{events.error.message}</p> : !events.data?.length ? <p className="empty-state">Noch keine Worker-Meldung. Prüfe, ob ein Ansible-Worker läuft und das Ziel erreichbar ist.</p> : <ol className="job-log">{events.data.map((event) => <li key={event.sequence}><span className="job-log-sequence">{event.sequence}</span><div><strong>{eventTitle(event)}</strong><p>{eventDetail(event)}</p></div><time className="muted">{new Date(event.created_at).toLocaleString()}</time></li>)}</ol>}</section>
  </>;
}

function eventTitle(event: AnsibleJobEvent) {
  const kind = event.event.kind;
  if (kind === "queued") return "Job eingereiht";
  if (kind === "status_changed") return `Status: ${event.event.status ?? "aktualisiert"}`;
  if (kind === "task_started") return `Task gestartet: ${event.event.task ?? "unbekannt"}`;
  if (kind === "task_finished") return `Task beendet: ${event.event.task ?? "unbekannt"}`;
  if (kind === "failed") return "Workflow fehlgeschlagen";
  return "Abgleich erforderlich";
}

function eventDetail(event: AnsibleJobEvent) {
  if (event.event.kind === "task_finished") return event.event.changed ? "Die Aufgabe hat Änderungen vorgenommen." : "Die Aufgabe wurde ohne Änderungen beendet.";
  if (event.event.kind === "failed") return `Fehlercode: ${event.event.code ?? "unbekannt"}`;
  if (event.event.kind === "reconcile_required") return "Der Status muss durch einen kontrollierten Abgleich bestätigt werden.";
  return "Vom Workflow-Controller erfasst.";
}

function jobGuidance(status: string, event: AnsibleJobEvent | undefined) {
  if (status === "queued") return { level: "info", title: "Wartet auf den Ansible-Worker", detail: "Der Controller hat den Job angenommen, aber noch keine Worker-Ausführung erhalten. Prüfe Worker, Zielverbindung und Secrets." };
  if (status === "failed") return { level: "danger", title: "Workflow fehlgeschlagen", detail: event?.event.code ? `Der Worker meldet den Fehlercode „${event.event.code}“. Die betroffene Aufgabe steht im Protokoll.` : "Der Worker hat einen Fehler gemeldet. Prüfe den letzten Protokolleintrag und die Zielverbindung." };
  if (status === "reconcile_required") return { level: "danger", title: "Abgleich erforderlich", detail: "Die Ausführung wurde unterbrochen oder ihr Ergebnis ist unklar. Starte einen Reconcile-Workflow, bevor du weitere Änderungen ausführst." };
  if (status === "succeeded") return { level: "success", title: "Workflow abgeschlossen", detail: "Alle vom Worker gemeldeten Schritte sind im Protokoll dokumentiert." };
  return { level: "info", title: "Workflow wird ausgeführt", detail: "Das Protokoll aktualisiert sich automatisch, sobald der Worker einen Schritt meldet." };
}
