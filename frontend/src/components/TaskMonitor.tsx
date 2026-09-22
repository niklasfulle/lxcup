import { Link } from "react-router-dom";
import type { ApiEvent } from "../api";

export type GlobalEvent = { id: string; event: ApiEvent; receivedAt: string };

export function TaskMonitor({ events, onClear }: { events: GlobalEvent[]; onClear: () => void }) {
  return <section className="task-monitor" aria-label="Globale Aufgaben und Ereignisse">
    <div className="section-heading"><div><h2>Aktivität</h2><span className="muted">{events.length} Ereignisse</span></div><button className="secondary-button" type="button" onClick={onClear} disabled={!events.length}>Aktivität leeren</button></div>
    {!events.length ? <p className="muted">Noch keine laufenden Aufgaben.</p> : <ul className="activity-list">{events.slice(0, 8).map((item) => <li key={item.id}><EventBadge event={item.event} /><div><strong>{eventTitle(item.event)}</strong><span className="muted">{new Date(item.receivedAt).toLocaleTimeString()}</span></div><EventLink event={item.event} /></li>)}</ul>}
  </section>;
}

function EventBadge({ event }: { event: ApiEvent }) {
  const type = event.type.toLowerCase();
  return <span className={`activity-badge ${type}`}>{event.type.slice(0, 1)}</span>;
}

function eventTitle(event: ApiEvent): string {
  if (event.type === "Status") return `${event.payload.resource} · ${event.payload.state}`;
  if (event.type === "Task") return `Task · ${event.payload.state}`;
  if (event.type === "Log") return event.payload.message.slice(0, 180);
  return event.payload.message.slice(0, 180);
}

function EventLink({ event }: { event: ApiEvent }) {
  if (event.type !== "Status") return null;
  if (event.payload.resource === "container_action") return <Link to={`/containers/${event.payload.resource_id}`}>Öffnen</Link>;
  if (event.payload.resource === "ansible_job") return <Link to={`/workflows/${event.payload.resource_id}`}>Öffnen</Link>;
  return null;
}
