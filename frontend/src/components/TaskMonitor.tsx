import { cn, ui } from "../ui";
import { Link } from "react-router-dom";
import type { ApiEvent } from "../api";

export type GlobalEvent = { id: string; event: ApiEvent; receivedAt: string };

type TaskMonitorProps = Readonly<{ events: GlobalEvent[]; onClear: () => void }>;

export function TaskMonitor({ events, onClear }: TaskMonitorProps) {
  return <section className={ui.taskMonitor} aria-label="Globale Aufgaben und Ereignisse">
    <div className={ui.sectionHeading}><div><h2>Aktivität</h2><span className={ui.muted}>{events.length} Ereignisse</span></div><button className={ui.button} type="button" onClick={onClear} disabled={events.length === 0}>Aktivität leeren</button></div>
    {events.length === 0 ? <p className={ui.muted}>Noch keine laufenden Aufgaben.</p> : <ul className={ui.activityList}>{events.slice(0, 8).map((item) => <li key={item.id}><EventBadge event={item.event} /><div><strong>{eventTitle(item.event)}</strong><span className={ui.muted}>{new Date(item.receivedAt).toLocaleTimeString()}</span></div><EventLink event={item.event} /></li>)}</ul>}
  </section>;
}

function EventBadge({ event }: Readonly<{ event: ApiEvent }>) {
  const type = event.type.toLowerCase();
  return <span className={cn(ui.activityBadge, type === "error" && ui.activityBadgeError)}>{event.type.slice(0, 1)}</span>;
}

function eventTitle(event: ApiEvent): string {
  if (event.type === "Status") return `${event.payload.resource} · ${event.payload.state}`;
  if (event.type === "Task") return `Task · ${event.payload.state}`;
  if (event.type === "Log") return event.payload.message.slice(0, 180);
  return event.payload.message.slice(0, 180);
}

function EventLink({ event }: Readonly<{ event: ApiEvent }>) {
  if (event.type === "Status" && event.payload.resource === "container_action") return <Link to={`/containers/${event.payload.resource_id}`}>Öffnen</Link>;
  if (event.type === "Status" && event.payload.resource === "ansible_job") return <Link to={`/workflows/${event.payload.resource_id}`}>Öffnen</Link>;
  return null;
}
