import { cn } from "../classnames";
import { Link } from "react-router-dom";
import type { ApiEvent } from "../api";
import { jobStatusBadgeClass, jobStatusLabel } from "../jobStatus";
import { ActivityIcon, activityIconForOperation, activityStatusTone, type ActivityIconName } from "./ActivityIcon";

export type GlobalEvent = {
  id: string;
  event: ApiEvent;
  receivedAt: string;
  jobDetails?: { operationLabel: string; resourceName: string };
};

type TaskMonitorProps = Readonly<{
  events: GlobalEvent[];
  onClear: () => void;
  collapsed: boolean;
  onToggle: () => void;
}>;

export function TaskMonitor({ events, onClear, collapsed, onToggle }: TaskMonitorProps) {
  return <>
    <aside
      className={cn(
        "fixed top-11 right-0 bottom-0 z-40 flex w-96 max-w-[calc(100vw-1rem)] min-h-0 flex-col overflow-hidden border-l border-[var(--line)] bg-[var(--panel)] p-3 text-[var(--ink)] shadow-xl transition-transform duration-300 ease-in-out motion-reduce:transition-none",
        collapsed ? "translate-x-full pointer-events-none" : "translate-x-0",
      )}
      aria-label="Aktivitäten"
      aria-hidden={collapsed}
      inert={collapsed}
    >
      <div className="mb-2 flex shrink-0 items-start justify-between gap-2 border-b border-[var(--line)] py-2">
        <div><h2>Aktivität</h2><span className="text-[var(--muted)]">{events.length} Ereignisse</span></div>
        <div className="flex gap-1">
          <button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onClear} disabled={events.length === 0} aria-label="Aktivität leeren">Leeren</button>
          <button className="inline-flex items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-2 py-1.5 text-xs font-medium text-[var(--ink)] hover:bg-[var(--primary-soft)] hover:text-lxcup-primary disabled:cursor-not-allowed disabled:opacity-50" type="button" onClick={onToggle} aria-label="Aktivitäten ausblenden" title="Aktivitäten ausblenden">›</button>
        </div>
      </div>
      {events.length === 0 ? <p className="m-0 py-3 text-xs text-[var(--muted)]">Noch keine laufenden Aufgaben.</p> : <ul className="m-0 min-h-0 flex-1 list-none overflow-y-auto [scrollbar-color:var(--line)_transparent] [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1.5 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-[var(--line)] [&::-webkit-scrollbar-track]:bg-transparent p-0">{events.slice(0, 8).map((item) => <li className="grid min-w-0 grid-cols-[1.75rem_minmax(0,1fr)] items-start gap-x-2.5 gap-y-1.5 border-b border-[var(--line)] py-2.5 last:border-0" key={item.id}>
        <span className="row-span-2 mt-0.5"><EventBadge item={item} /></span>
        <strong className="min-w-0 break-words text-xs leading-4 [overflow-wrap:anywhere]">{eventTitle(item)}</strong>
        <div className="flex min-w-0 items-center justify-between gap-2">
          <time className="shrink-0 text-[11px] leading-4 text-[var(--muted)]" dateTime={item.receivedAt}>{new Date(item.receivedAt).toLocaleTimeString()}</time>
          <div className="flex shrink-0 items-center gap-2"><JobStatus event={item.event} /><EventLink event={item.event} /></div>
        </div>
      </li>)}</ul>}
    </aside>
    <div className={cn("fixed right-0 top-1/2 z-50 -translate-y-1/2 transition-opacity duration-200 ease-in-out motion-reduce:transition-none", collapsed ? "opacity-100" : "pointer-events-none opacity-0")} aria-hidden={!collapsed}>
      <button className="flex h-16 w-7 flex-col items-center justify-center gap-1 rounded-l-md border border-r-0 border-[var(--line)] bg-[var(--panel)] p-0 text-xs font-semibold text-[var(--ink)] shadow-md hover:bg-[var(--primary-soft)]" type="button" onClick={onToggle} aria-label="Aktivitäten einblenden" title="Aktivitäten einblenden" tabIndex={collapsed ? 0 : -1}>
        <span aria-hidden="true">‹</span>
        {events.length > 0 ? <span className="text-[10px]">{events.length}</span> : null}
      </button>
    </div>
  </>;
}

function EventBadge({ item }: Readonly<{ item: GlobalEvent }>) {
  const appearance = activityAppearance(item);
  return <span className={cn("grid h-7 w-7 place-items-center rounded-full border", appearance.tone)} aria-hidden="true" title={appearance.label}>
    <ActivityIcon name={appearance.icon} />
  </span>;
}

function activityAppearance(item: GlobalEvent): Readonly<{ icon: ActivityIconName; label: string; tone: string }> {
  const { event } = item;
  if (event.type === "Error") return { icon: "error", label: "Fehlerereignis", tone: "border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]" };
  if (event.type === "Task") return { icon: "task", label: "Aufgabenschritt", tone: "border-[var(--primary)]/40 bg-[var(--primary-soft)] text-lxcup-primary" };
  if (event.type === "Log") return { icon: "log", label: "Worker-Log", tone: "border-[var(--line)] bg-[var(--paper-muted)] text-[var(--muted)]" };
  let icon: ActivityIconName;
  let label: string;
  if (event.payload.resource === "ansible_job") {
    icon = activityIconForOperation(item.jobDetails?.operationLabel);
    label = item.jobDetails?.operationLabel ?? "Workflow";
  } else if (event.payload.resource === "container_action") {
    icon = "lxc";
    label = "LXC-Aktion";
  } else {
    icon = "server";
    label = `Status ${event.payload.resource}`;
  }
  return { icon, label, tone: activityStatusTone(event.payload.state) };
}

function eventTitle(item: GlobalEvent): string {
  const { event } = item;
  if (event.type === "Status" && event.payload.resource === "ansible_job") {
    return item.jobDetails ? `${item.jobDetails.operationLabel} · ${item.jobDetails.resourceName}` : "Ansible-Job";
  }
  if (event.type === "Status") return `${event.payload.resource} · ${event.payload.state}`;
  if (event.type === "Task") return `Task · ${event.payload.state}`;
  if (event.type === "Log") return event.payload.message.slice(0, 180);
  return event.payload.message.slice(0, 180);
}

function JobStatus({ event }: Readonly<{ event: ApiEvent }>) {
  if (event.type !== "Status" || event.payload.resource !== "ansible_job") return null;
  const statusLabel = jobStatusLabel(event.payload.state);
  return <output className={cn("inline-flex shrink-0 items-center rounded px-1.5 py-0.5 text-[10px] font-semibold", jobStatusBadgeClass(event.payload.state))} aria-label={`Jobstatus: ${statusLabel} (${event.payload.state})`} title={`Jobstatus: ${event.payload.state}`}>
    {statusLabel}
  </output>;
}

function EventLink({ event }: Readonly<{ event: ApiEvent }>) {
  if (event.type === "Status" && event.payload.resource === "container_action") return <Link className="self-center text-[10px] font-semibold text-lxcup-primary hover:underline" to={`/containers/${event.payload.resource_id}`}>Öffnen</Link>;
  if (event.type === "Status" && event.payload.resource === "ansible_job") return <Link className="self-center text-[10px] font-semibold text-lxcup-primary hover:underline" to={`/workflows/${event.payload.resource_id}`}>Öffnen</Link>;
  return null;
}
