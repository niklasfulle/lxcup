import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createSchedule, setScheduleEnabled } from "../api";
import { queryKeys, useSchedules, useTargets, useUpdatePolicies } from "../queries";
import { cn } from "../classnames";

const panelClass = "border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)] shadow-sm";
const fieldClass = "grid content-start min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]";
const inputClass = "block h-10 w-full rounded-sm border border-[var(--line)] bg-[var(--paper)] px-3 py-2 text-sm font-normal leading-5 text-[var(--ink)] transition-colors focus:border-[var(--primary)] focus:ring-2 focus:ring-[var(--primary)]/20";
const primaryButtonClass = "inline-flex min-h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 py-2 text-sm font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50";

export function SchedulesPage() {
  const targets = useTargets();
  const schedules = useSchedules();
  const policies = useUpdatePolicies();
  const queryClient = useQueryClient();
  const [id, setId] = useState("");
  const [targetId, setTargetId] = useState("");
  const [operation, setOperation] = useState("collect_package_inventory");
  const [everyMinutes, setEveryMinutes] = useState("60");
  const [timezone, setTimezone] = useState("Europe/Berlin");
  const [policyId, setPolicyId] = useState("");
  const [thresholdEnabled, setThresholdEnabled] = useState(false);
  const [thresholdMetric, setThresholdMetric] = useState<"cpu_basis_points" | "memory_basis_points" | "storage_basis_points" | "heartbeat_age_seconds">("cpu_basis_points");
  const [thresholdOperator, setThresholdOperator] = useState<"greater_than_or_equal" | "less_than_or_equal">("greater_than_or_equal");
  const [thresholdValue, setThresholdValue] = useState("8000");

  const mutation = useMutation({
    mutationFn: () => createSchedule({
      id,
      operation,
      timezone,
      target_ids: [targetId],
      every_minutes: Number(everyMinutes),
      enabled: true,
      threshold: thresholdEnabled ? { metric: thresholdMetric, operator: thresholdOperator, value: Number(thresholdValue) } : null,
      policy_id: operation === "update_packages" ? policyId : null,
    }),
    onSuccess: () => { setId(""); void queryClient.invalidateQueries({ queryKey: queryKeys.schedules }); },
  });
  const enabledMutation = useMutation({
    mutationFn: ({ scheduleId, enabled }: { scheduleId: string; enabled: boolean }) => setScheduleEnabled(scheduleId, enabled),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: queryKeys.schedules }); },
  });

  const canSubmit = id.trim() !== "" && targetId !== "" && Number(everyMinutes) > 0 && (operation !== "update_packages" || policyId !== "");
  const targetList = targets.data ?? [];
  return <div className="grid gap-5">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4 max-[720px]:items-start">
      <div>
        <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Automatisierung</p>
        <h1 className="mb-1">Zeitpläne</h1>
        <p className="mb-0 text-sm text-[var(--muted)]">Wiederkehrende Aufgaben planen und Paketupdates mit klaren Regeln absichern.</p>
      </div>
      <div className="flex shrink-0 items-center" aria-label="Zeitplanübersicht">
        <span className="inline-flex min-h-9 items-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs text-[var(--muted)]"><strong className="mr-1 text-[var(--ink)]">{schedules.data?.length ?? 0}</strong>Zeitpläne</span>
      </div>
    </header>

    <section className={panelClass} aria-labelledby="schedule-create-title">
        <SectionHeading title="Zeitplan erstellen" description="Lege fest, welcher Job für welches Ziel wiederholt ausgeführt wird." id="schedule-create-title" />
        <form className="grid gap-4" onSubmit={(event) => { event.preventDefault(); if (canSubmit) mutation.mutate(); }}>
          <div className="grid gap-x-4 gap-y-4 sm:grid-cols-2 xl:grid-cols-3">
            <label className={fieldClass}><span>Name</span><input className={inputClass} value={id} onChange={(event) => setId(event.target.value)} placeholder="nightly-inventory" required /></label>
            <label className={fieldClass}><span>Ziel</span><select className={inputClass} value={targetId} onChange={(event) => { setTargetId(event.target.value); if (targetList.find((target) => target.id === event.target.value)?.kind !== "lxc") setOperation((current) => current === "docker_discovery" ? "collect_package_inventory" : current); }} required><option value="">Ziel auswählen</option>{targetList.map((target) => <option key={target.id} value={target.id}>{target.name}</option>)}</select></label>
            <label className={fieldClass}><span>Aufgabe</span><select className={inputClass} value={operation} onChange={(event) => setOperation(event.target.value)}><option value="collect_package_inventory">Paketinventar erfassen</option><option value="health_check">Healthcheck</option><option value="update_packages">Pakete aktualisieren</option>{targetList.find((target) => target.id === targetId)?.kind === "lxc" ? <option value="docker_discovery">Docker-Container erkennen</option> : null}</select></label>
            <label className={fieldClass}><span>Intervall in Minuten</span><input className={inputClass} type="number" min="1" max="10080" value={everyMinutes} onChange={(event) => setEveryMinutes(event.target.value)} required /><span className="font-normal">Zum Beispiel 60 für eine stündliche Ausführung.</span></label>
            <label className={cn(fieldClass, operation === "update_packages" ? "" : "xl:col-span-2")}><span>Zeitzone</span><input className={inputClass} value={timezone} onChange={(event) => setTimezone(event.target.value)} required /><span className="font-normal">Gilt für die Berechnung der nächsten Ausführung.</span></label>
            {operation === "update_packages" ? <label className={fieldClass}><span>Update-Policy</span><select className={inputClass} value={policyId} onChange={(event) => setPolicyId(event.target.value)} required><option value="">Policy auswählen</option>{(policies.data ?? []).filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id}</option>)}</select><Link className="w-fit font-semibold text-lxcup-primary hover:underline" to="/update-policies">Policies verwalten →</Link></label> : null}
          </div>

          <fieldset className="grid gap-3 border border-[var(--line)] bg-[var(--paper-muted)] p-4">
            <legend className="px-1 text-xs font-bold text-[var(--ink)]">Optionale Ausführungsbedingung</legend>
            <label className="flex cursor-pointer items-center gap-3 text-sm font-semibold text-[var(--ink)]">
              <input className="!h-4 !w-4 !shrink-0 accent-lxcup-primary" aria-label="Schwellwert aktivieren" type="checkbox" checked={thresholdEnabled} onChange={(event) => setThresholdEnabled(event.target.checked)} />
              <span>Nur bei erreichtem Schwellwert starten</span>
            </label>
            {thresholdEnabled ? <div className="grid gap-3 sm:grid-cols-3">
              <label className={fieldClass}><span>Metrik</span><select className={inputClass} value={thresholdMetric} onChange={(event) => setThresholdMetric(event.target.value as typeof thresholdMetric)}><option value="cpu_basis_points">CPU-Auslastung</option><option value="memory_basis_points">RAM-Auslastung</option><option value="storage_basis_points">Speicherauslastung</option><option value="heartbeat_age_seconds">Heartbeat-Alter</option></select></label>
              <label className={fieldClass}><span>Bedingung</span><select className={inputClass} value={thresholdOperator} onChange={(event) => setThresholdOperator(event.target.value as typeof thresholdOperator)}><option value="greater_than_or_equal">größer oder gleich</option><option value="less_than_or_equal">kleiner oder gleich</option></select></label>
              <label className={fieldClass}><span>Grenzwert</span><input className={inputClass} type="number" min="0" value={thresholdValue} onChange={(event) => setThresholdValue(event.target.value)} /></label>
            </div> : <p className="m-0 text-xs text-[var(--muted)]">Ohne Schwellwert läuft der Job ausschließlich nach dem gewählten Intervall.</p>}
          </fieldset>

          {operation === "update_packages" && !policies.data?.some((policy) => policy.enabled) ? <output className="m-0 block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Es gibt noch keine aktive Update-Policy. <Link className="font-semibold text-lxcup-primary hover:underline" to="/update-policies">Zuerst eine Policy erstellen →</Link></output> : null}

          {mutation.error ? <p className="m-0 font-semibold text-[var(--error)]" role="alert">{mutation.error.message}</p> : null}
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--line)] pt-4">
            <p className="m-0 text-xs text-[var(--muted)]">{targetList.length ? `${targetList.length} Ziele verfügbar` : "Lege zuerst ein Ziel an, um einen Zeitplan zu erstellen."}</p>
            <button className={primaryButtonClass} type="submit" disabled={!canSubmit || mutation.isPending || !targetList.length}>{mutation.isPending ? "Wird gespeichert…" : "Zeitplan speichern"}<span aria-hidden="true">→</span></button>
          </div>
        </form>
    </section>

    <section className={panelClass} aria-labelledby="active-schedules-title">
      <div className="mb-4 flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-4">
        <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Übersicht</p><h2 id="active-schedules-title" className="mb-1">Zeitplan-Ausführungen</h2><p className="mb-0 text-sm text-[var(--muted)]">Nächste Läufe, letzte Ergebnisse und Fehler auf einen Blick.</p></div>
        <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-3 py-1.5 text-xs font-semibold text-[var(--muted)]">{schedules.data?.length ?? 0} gesamt</span>
      </div>
      {enabledMutation.error ? <p className="mb-3 font-semibold text-[var(--error)]" role="alert">{enabledMutation.error.message}</p> : null}
      <ScheduleContent schedules={schedules} targets={targetList} pending={enabledMutation.isPending} onToggle={(scheduleId, enabled) => enabledMutation.mutate({ scheduleId, enabled })} />
    </section>
  </div>;
}

function SectionHeading({ title, description, id }: Readonly<{ title: string; description: string; id: string }>) {
  return <div className="mb-5 flex gap-3 border-b border-[var(--line)] pb-4">
    <div><h2 id={id} className="mb-1">{title}</h2><p className="mb-0 text-sm leading-relaxed text-[var(--muted)]">{description}</p></div>
  </div>;
}

function ScheduleContent({ schedules, targets, onToggle, pending }: Readonly<{ schedules: ReturnType<typeof useSchedules>; targets: NonNullable<ReturnType<typeof useTargets>["data"]>; onToggle: (scheduleId: string, enabled: boolean) => void; pending: boolean }>) {
  if (schedules.isLoading) return <p className="m-0 py-8 text-center text-sm text-[var(--muted)]">Zeitpläne werden geladen…</p>;
  if (schedules.error) return <p className="m-0 py-4 font-semibold text-[var(--error)]" role="alert">{schedules.error.message}</p>;
  if (!schedules.data?.length) return <div className="grid min-h-36 place-items-center border border-dashed border-[var(--line)] bg-[var(--paper-muted)] px-4 py-8 text-center"><div><span className="mx-auto mb-3 grid h-9 w-9 place-items-center border border-[var(--line)] bg-[var(--panel)] text-lg text-[var(--muted)]" aria-hidden="true">↻</span><strong className="block text-sm">Noch keine Zeitpläne</strong><span className="mt-1 block text-xs text-[var(--muted)]">Erstelle oben einen Zeitplan, um wiederkehrende Jobs automatisch auszuführen.</span></div></div>;
  return <div className="overflow-x-auto">
    <table><thead><tr><th>Name</th><th>Aufgabe</th><th>Ziel</th><th>Intervall</th><th>Nächster Lauf</th><th>Letzter Lauf</th><th>Status</th><th>Letzter Fehler</th></tr></thead><tbody>
      {schedules.data.map((schedule) => <tr key={schedule.id}>
        <td><strong>{schedule.id}</strong><span className="mt-1 block text-xs text-[var(--muted)]">{schedule.timezone}</span></td>
        <td>{operationLabel(schedule.operation)}</td>
        <td>{schedule.target_ids.map((targetId) => targets.find((target) => target.id === targetId)?.name ?? targetId).join(", ")}</td>
        <td>Alle {schedule.every_minutes} Min.</td>
        <td>{new Date(schedule.next_run_at).toLocaleString()}</td>
        <td>{schedule.last_run_at ? new Date(schedule.last_run_at).toLocaleString() : "Noch nicht ausgeführt"}</td>
        <td><span className={cn("inline-flex items-center px-2 py-1 text-xs font-bold", schedule.enabled ? "bg-[var(--success-soft)] text-[var(--success)]" : "bg-[var(--paper-muted)] text-[var(--muted)]")}>{schedule.enabled ? "Aktiv" : "Pausiert"}</span><button className="ml-2 border border-[var(--line)] px-2 py-1 text-xs font-semibold hover:border-lxcup-primary disabled:opacity-50" type="button" disabled={pending} onClick={() => onToggle(schedule.id, !schedule.enabled)}>{schedule.enabled ? "Pausieren" : "Aktivieren"}</button></td>
        <td>{schedule.last_error ? <span className="font-semibold text-[var(--error)]">{schedule.last_error}</span> : "—"}</td>
      </tr>)}
    </tbody></table>
  </div>;
}

function operationLabel(operation: string) {
  const labels: Record<string, string> = {
    collect_package_inventory: "Paketinventar",
    health_check: "Healthcheck",
    update_packages: "Paketupdate",
    docker_discovery: "Docker-Container erkennen",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}
