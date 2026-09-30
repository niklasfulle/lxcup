import { useState, type SubmitEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { ApiError, listUserAudit, type UserAuditDto, type UserAuditFilters } from "../api";
import { queryKeys } from "../queries";

const PAGE_SIZE = 50;

export default function AuditLogPage() {
  const [actorDraft, setActorDraft] = useState("");
  const [actionDraft, setActionDraft] = useState("");
  const [resourceDraft, setResourceDraft] = useState("");
  const [fromDraft, setFromDraft] = useState("");
  const [untilDraft, setUntilDraft] = useState("");
  const [filters, setFilters] = useState<UserAuditFilters>({ limit: PAGE_SIZE, offset: 0 });
  const audit = useQuery({
    queryKey: [...queryKeys.userAudit, filters],
    queryFn: ({ signal }) => listUserAudit(filters, signal),
  });

  const applyFilters = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    setFilters({
      limit: PAGE_SIZE,
      offset: 0,
      ...(actorDraft.trim() ? { actor_username: actorDraft.trim() } : {}),
      ...(actionDraft.trim() ? { action: actionDraft.trim() } : {}),
      ...(resourceDraft.trim() ? { resource: resourceDraft.trim() } : {}),
      ...(fromDraft ? { since: dateStart(fromDraft) } : {}),
      ...(untilDraft ? { until: dateStart(addDays(untilDraft, 1)) } : {}),
    });
  };

  const page = audit.data;
  let pageRangeLabel = "";
  if (page) {
    const pageStart = page.total === 0 ? 0 : page.offset + 1;
    pageRangeLabel = `${pageStart}–${page.offset + page.events.length} von ${page.total}`;
  }
  const hasPrevious = (filters.offset ?? 0) > 0;
  const hasNext = page !== undefined && page.offset + page.events.length < page.total;
  return <div className="grid gap-4">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4">
      <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Administration · Sicherheit</p><h1 className="mb-1">Aktivitätsprotokoll</h1><p className="mb-0 text-sm text-[var(--muted)]">Nachvollziehbare Anmeldungen und Änderungen durch Benutzer. Nur Admins können dieses Protokoll einsehen.</p></div>
      <span className="border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs text-[var(--muted)]"><strong className="text-[var(--ink)]">{page?.total ?? 0}</strong> Ereignisse</span>
    </header>
    <form className="grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 sm:grid-cols-2 xl:grid-cols-5" aria-label="Audit-Filter" onSubmit={applyFilters}>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Benutzer<input value={actorDraft} onChange={(event) => setActorDraft(event.target.value)} placeholder="Benutzername" /></label>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Aktion<input value={actionDraft} onChange={(event) => setActionDraft(event.target.value)} placeholder="z. B. user.created" /></label>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Ressource<input value={resourceDraft} onChange={(event) => setResourceDraft(event.target.value)} placeholder="Typ oder ID" /></label>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Von<input type="date" value={fromDraft} onChange={(event) => setFromDraft(event.target.value)} /></label>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Bis<input type="date" value={untilDraft} onChange={(event) => setUntilDraft(event.target.value)} /></label>
      <div className="flex flex-wrap gap-2 sm:col-span-2 xl:col-span-5">
        <button className="inline-flex min-h-9 items-center border border-lxcup-primary bg-lxcup-primary px-3 text-xs font-semibold text-white hover:bg-blue-700" type="submit">Filter anwenden</button>
        <button className="inline-flex min-h-9 items-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold text-[var(--ink)] hover:bg-[var(--primary-soft)]" type="button" onClick={() => { setActorDraft(""); setActionDraft(""); setResourceDraft(""); setFromDraft(""); setUntilDraft(""); setFilters({ limit: PAGE_SIZE, offset: 0 }); }}>Filter zurücksetzen</button>
      </div>
    </form>
    <section className="grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-label="Benutzeraktivitäten">
      {audit.isPending ? <output className="m-0 text-sm text-[var(--muted)]">Aktivitäten werden geladen …</output> : null}
      {audit.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{audit.error instanceof ApiError ? audit.error.message : "Das Protokoll konnte nicht geladen werden."}</p> : null}
      {page?.events.length === 0 ? <p className="m-0 border border-dashed border-[var(--line)] p-6 text-center text-sm text-[var(--muted)]">Keine Aktivitäten für diese Filter.</p> : null}
      {page && page.events.length > 0 ? <div className="overflow-x-auto"><table><thead><tr><th>Zeitpunkt</th><th>Benutzer</th><th>Aktion</th><th>Ressource</th><th>Ergebnis</th></tr></thead><tbody>{page.events.map((event) => <AuditEventRow key={event.id} event={event} />)}</tbody></table></div> : null}
      <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--line)] pt-3 text-xs text-[var(--muted)]">
        <span>{pageRangeLabel}</span>
        <div className="flex gap-2"><button className="border border-[var(--line)] px-3 py-1.5 text-[var(--ink)] disabled:opacity-50" type="button" disabled={!hasPrevious || audit.isFetching} onClick={() => setFilters((current) => ({ ...current, offset: Math.max(0, current.offset - PAGE_SIZE) }))}>Zurück</button><button className="border border-[var(--line)] px-3 py-1.5 text-[var(--ink)] disabled:opacity-50" type="button" disabled={!hasNext || audit.isFetching} onClick={() => setFilters((current) => ({ ...current, offset: current.offset + PAGE_SIZE }))}>Weiter</button></div>
      </div>
    </section>
  </div>;
}

function AuditEventRow({ event }: Readonly<{ event: UserAuditDto }>) {
  const roleLabel = event.actor_role === "admin" ? "Admin" : "User";
  const resourceId = event.resource_id ? <small className="ml-1 text-[var(--muted)]">{event.resource_id}</small> : null;
  const pendingTitle = event.status_code === 102
    ? "Der Request wurde begonnen, aber sein Ergebnis ist noch nicht gespeichert."
    : undefined;
  return <tr>
    <td className="whitespace-nowrap">{formatDate(event.created_at)}</td>
    <td><strong>{event.actor_username}</strong><small className="mt-1 block text-[var(--muted)]">{roleLabel}</small></td>
    <td><code className="text-xs">{event.action}</code></td>
    <td>{event.resource_type}{resourceId}</td>
    <td><span className={auditOutcomeClass(event.status_code)} title={pendingTitle}>{auditOutcomeLabel(event.status_code)}</span></td>
  </tr>;
}

function dateStart(value: string) {
  return new Date(`${value}T00:00:00.000Z`).toISOString();
}

function addDays(value: string, days: number) {
  const date = new Date(`${value}T00:00:00.000Z`);
  date.setUTCDate(date.getUTCDate() + days);
  return date.toISOString().slice(0, 10);
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat("de-DE", { dateStyle: "medium", timeStyle: "medium" }).format(new Date(value));
}

function auditOutcomeLabel(statusCode: number) {
  return statusCode === 102 ? "Ausstehend" : statusCode.toString();
}

function auditOutcomeClass(statusCode: number) {
  if (statusCode === 102) return "text-[var(--warning)]";
  return statusCode < 400 ? "text-[var(--success)]" : "text-[var(--error)]";
}
