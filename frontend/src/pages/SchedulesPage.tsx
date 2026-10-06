import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createSchedule, listSecrets, setScheduleEnabled, type CreateScheduleRequest, type SecretMetadata } from "../api";
import { queryKeys, useSchedules, useTargets, useUpdatePolicies } from "../queries";
import { cn } from "../classnames";
import { targetDisplayLabel } from "../targetLabel";

const panelClass = "border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)] shadow-sm";
const fieldClass = "grid content-start min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]";
const inputClass = "block h-10 w-full rounded-sm border border-[var(--line)] bg-[var(--paper)] px-3 py-2 text-sm font-normal leading-5 text-[var(--ink)] transition-colors focus:border-[var(--primary)] focus:ring-2 focus:ring-[var(--primary)]/20";
const primaryButtonClass = "inline-flex min-h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 py-2 text-sm font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50";
type ThresholdMetric = "cpu_basis_points" | "memory_basis_points" | "storage_basis_points" | "heartbeat_age_seconds";

export function SchedulesPage({ isAdmin = false }: Readonly<{ isAdmin?: boolean }>) {
  const targets = useTargets();
  const schedules = useSchedules();
  const policies = useUpdatePolicies();
  const secrets = useQuery({ queryKey: queryKeys.secrets, queryFn: ({ signal }) => listSecrets(signal), enabled: isAdmin });
  const queryClient = useQueryClient();
  const [id, setId] = useState("");
  const [targetId, setTargetId] = useState("");
  const [operation, setOperation] = useState("collect_package_inventory");
  const [everyMinutes, setEveryMinutes] = useState("60");
  const [timezone, setTimezone] = useState("Europe/Berlin");
  const [policyId, setPolicyId] = useState("");
  const [backupSecretRef, setBackupSecretRef] = useState("");
  const [thresholdEnabled, setThresholdEnabled] = useState(false);
  const [thresholdMetric, setThresholdMetric] = useState<ThresholdMetric>("cpu_basis_points");
  const [thresholdOperator, setThresholdOperator] = useState<"greater_than_or_equal" | "less_than_or_equal">("greater_than_or_equal");
  const [thresholdValue, setThresholdValue] = useState("8000");
  const isBackupOperation = operation === "create_backup";

  const mutation = useMutation({
    mutationFn: () => createSchedule(buildScheduleRequest({
      id, operation, timezone, targetId, everyMinutes, thresholdEnabled,
      thresholdMetric, thresholdOperator, thresholdValue, policyId, backupSecretRef,
    })),
    onSuccess: () => { setId(""); void queryClient.invalidateQueries({ queryKey: queryKeys.schedules }); },
  });
  const enabledMutation = useMutation({
    mutationFn: ({ scheduleId, enabled }: { scheduleId: string; enabled: boolean }) => setScheduleEnabled(scheduleId, enabled),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: queryKeys.schedules }); },
  });

  const canSubmit = canSubmitSchedule({ id, operation, backupSecretRef, targetId, everyMinutes, policyId });
  const targetList = targets.data ?? [];
  const backupSecrets = (secrets.data ?? []).filter(({ metadata }) => metadata.status === "active" && metadata.metadata.kind === "backup_passphrase" && metadata.metadata.scope.type === "global");
  const handleTargetChange = (nextTargetId: string) => {
    setTargetId(nextTargetId);
    if (targetList.find((item) => item.id === nextTargetId)?.kind !== "lxc") {
      setOperation((current) => current === "docker_discovery" ? "collect_package_inventory" : current);
    }
  };
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
            <ScheduleTargetField hidden={isBackupOperation} targetId={targetId} targets={targetList} onChange={handleTargetChange} />
            <ScheduleOperationField operation={operation} isAdmin={isAdmin} hasLxcTarget={targetList.some((target) => target.id === targetId && target.kind === "lxc")} onChange={(value) => { setOperation(value); setBackupSecretRef(""); }} />
            <ScheduleIntervalField value={everyMinutes} isBackup={isBackupOperation} onChange={setEveryMinutes} />
            <ScheduleContextField isBackup={isBackupOperation} timezone={timezone} onTimezoneChange={setTimezone} secrets={backupSecrets} secretRef={backupSecretRef} onSecretChange={setBackupSecretRef} secretsLoading={secrets.isLoading} secretsError={secrets.error} />
            <SchedulePolicyField visible={operation === "update_packages"} policies={policies.data ?? []} policyId={policyId} onChange={setPolicyId} />
          </div>

          <ScheduleExecutionOptions isBackup={isBackupOperation} thresholdEnabled={thresholdEnabled} onThresholdEnabled={setThresholdEnabled} metric={thresholdMetric} onMetricChange={setThresholdMetric} operator={thresholdOperator} onOperatorChange={setThresholdOperator} value={thresholdValue} onValueChange={setThresholdValue} />

           {operation === "update_packages" && !policies.data?.some((policy) => policy.enabled) ? <output className="m-0 block border border-[var(--warning)] bg-[var(--warning-soft)] p-3 text-sm text-[var(--ink)]">Es gibt noch keine aktive Update-Policy. <Link className="inline-flex items-center gap-1 font-semibold text-lxcup-primary hover:underline" to="/update-policies">Zuerst eine Policy erstellen <span className="text-base leading-none" aria-hidden="true">→</span></Link></output> : null}

          {mutation.error ? <p className="m-0 font-semibold text-[var(--error)]" role="alert">{mutation.error.message}</p> : null}
          <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--line)] pt-4">
            <p className="m-0 text-xs text-[var(--muted)]">{scheduleAvailabilityLabel(isBackupOperation, targetList.length)}</p>
             <button className={primaryButtonClass} type="submit" disabled={!canSubmit || mutation.isPending || (!isBackupOperation && !targetList.length)}>{mutation.isPending ? "Wird gespeichert…" : "Zeitplan speichern"}<span className="text-base leading-none" aria-hidden="true">→</span></button>
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

type ScheduleTarget = NonNullable<ReturnType<typeof useTargets>["data"]>[number];

function buildScheduleRequest(values: {
  id: string; operation: string; timezone: string; targetId: string; everyMinutes: string;
  thresholdEnabled: boolean; thresholdMetric: ThresholdMetric;
  thresholdOperator: "greater_than_or_equal" | "less_than_or_equal"; thresholdValue: string; policyId: string; backupSecretRef: string;
}): CreateScheduleRequest {
  const isBackup = values.operation === "create_backup";
  const isPackageUpdate = values.operation === "update_packages";
  return {
    id: values.id,
    operation: values.operation,
    timezone: isBackup ? "UTC" : values.timezone,
    target_ids: isBackup ? [] : [values.targetId],
    every_minutes: Number(values.everyMinutes),
    enabled: true,
    threshold: values.thresholdEnabled ? { metric: values.thresholdMetric, operator: values.thresholdOperator, value: Number(values.thresholdValue) } : null,
    policy_id: isPackageUpdate ? values.policyId : null,
    backup_secret_ref: isBackup ? values.backupSecretRef : null,
  };
}

function canSubmitSchedule(values: { id: string; operation: string; backupSecretRef: string; targetId: string; everyMinutes: string; policyId: string }) {
  const isBackup = values.operation === "create_backup";
  const hasRequiredTarget = isBackup ? values.backupSecretRef !== "" : values.targetId !== "";
  const minimumInterval = isBackup ? 60 : 1;
  const intervalIsValid = Number(values.everyMinutes) >= minimumInterval;
  const policyIsValid = values.operation !== "update_packages" || values.policyId !== "";
  return values.id.trim() !== "" && hasRequiredTarget && intervalIsValid && policyIsValid;
}

function ScheduleTargetField({ hidden, targetId, targets, onChange }: Readonly<{ hidden: boolean; targetId: string; targets: ScheduleTarget[]; onChange: (targetId: string) => void }>) {
  if (hidden) return null;
  return <label className={fieldClass}><span>Ziel</span><select className={inputClass} value={targetId} onChange={(event) => onChange(event.target.value)} required><option value="">Ziel auswählen</option>{targets.map((target) => <option key={target.id} value={target.id}>{targetDisplayLabel(target)}</option>)}</select></label>;
}

function ScheduleOperationField({ operation, isAdmin, hasLxcTarget, onChange }: Readonly<{ operation: string; isAdmin: boolean; hasLxcTarget: boolean; onChange: (operation: string) => void }>) {
  return <label className={fieldClass}><span>Aufgabe</span><select className={inputClass} value={operation} onChange={(event) => onChange(event.target.value)}><option value="collect_package_inventory">Paketinventar erfassen</option><option value="health_check">Healthcheck</option><option value="update_packages">Pakete aktualisieren</option>{hasLxcTarget ? <option value="docker_discovery">Docker-Container erkennen</option> : null}{isAdmin ? <option value="create_backup">Backup erstellen</option> : null}</select></label>;
}

function ScheduleIntervalField({ value, isBackup, onChange }: Readonly<{ value: string; isBackup: boolean; onChange: (value: string) => void }>) {
  const hint = isBackup ? "Mindestens 60 Minuten, höchstens 7 Tage zwischen Backups." : "Zum Beispiel 60 für eine stündliche Ausführung.";
  return <label className={fieldClass}><span>Intervall in Minuten</span><input className={inputClass} type="number" min={isBackup ? "60" : "1"} max="10080" value={value} onChange={(event) => onChange(event.target.value)} required /><span className="font-normal">{hint}</span></label>;
}

function ScheduleContextField({ isBackup, timezone, onTimezoneChange, secrets, secretRef, onSecretChange, secretsLoading, secretsError }: Readonly<{ isBackup: boolean; timezone: string; onTimezoneChange: (value: string) => void; secrets: SecretMetadata[]; secretRef: string; onSecretChange: (value: string) => void; secretsLoading: boolean; secretsError: Error | null }>) {
  if (isBackup) return <BackupSecretField secrets={secrets} secretRef={secretRef} onSecretChange={onSecretChange} loading={secretsLoading} error={secretsError} />;
  return <label className={fieldClass}><span>Zeitzone</span><input className={inputClass} value={timezone} onChange={(event) => onTimezoneChange(event.target.value)} required /><span className="font-normal">Gilt für die Berechnung der nächsten Ausführung.</span></label>;
}

function BackupSecretField({ secrets, secretRef, onSecretChange, loading, error }: Readonly<{ secrets: SecretMetadata[]; secretRef: string; onSecretChange: (value: string) => void; loading: boolean; error: Error | null }>) {
  const description = backupSecretDescription(secrets, loading, error);
  return <label className={fieldClass}><span>Passwort-Secret</span><select className={inputClass} value={secretRef} onChange={(event) => onSecretChange(event.target.value)} required disabled={loading || Boolean(error)}><option value="">Secret auswählen</option>{secrets.map(({ metadata }) => <option key={metadata.metadata.id} value={metadata.metadata.id}>{metadata.metadata.name}</option>)}</select><span className="font-normal">{description}</span>{error ? <span className="font-normal text-[var(--error)]" role="alert">{error.message}</span> : null}</label>;
}

function backupSecretDescription(secrets: SecretMetadata[], loading: boolean, error: Error | null) {
  if (loading) return "Secrets werden geladen…";
  if (error) return "Secrets konnten nicht geladen werden.";
  if (secrets.length > 0) return "Nur der Secret-Verweis wird im Zeitplan gespeichert. Er muss aktiv und global sein.";
  return <Link className="font-semibold text-lxcup-primary hover:underline" to="/secrets">Globales Backup-Passphrase-Secret erstellen</Link>;
}

function SchedulePolicyField({ visible, policies, policyId, onChange }: Readonly<{ visible: boolean; policies: NonNullable<ReturnType<typeof useUpdatePolicies>["data"]>; policyId: string; onChange: (value: string) => void }>) {
  if (!visible) return null;
  return <label className={fieldClass}><span>Update-Policy</span><select className={inputClass} value={policyId} onChange={(event) => onChange(event.target.value)} required><option value="">Policy auswählen</option>{policies.filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id}</option>)}</select><Link className="inline-flex w-fit items-center gap-1 font-semibold text-lxcup-primary hover:underline" to="/update-policies">Policies verwalten <span className="text-base leading-none" aria-hidden="true">→</span></Link></label>;
}

function ScheduleExecutionOptions({ isBackup, thresholdEnabled, onThresholdEnabled, metric, onMetricChange, operator, onOperatorChange, value, onValueChange }: Readonly<{ isBackup: boolean; thresholdEnabled: boolean; onThresholdEnabled: (enabled: boolean) => void; metric: ThresholdMetric; onMetricChange: (metric: ThresholdMetric) => void; operator: "greater_than_or_equal" | "less_than_or_equal"; onOperatorChange: (operator: "greater_than_or_equal" | "less_than_or_equal") => void; value: string; onValueChange: (value: string) => void }>) {
  if (isBackup) return <p className="m-0 border border-[var(--primary)]/30 bg-[var(--primary-soft)] p-3 text-sm text-[var(--ink)]">Geplante Backups werden mit dem ausgewählten Secret verschlüsselt und bleiben zum manuellen Download in der Backup-Liste. Das Secret wird nicht in Aktivitäten oder Zeitplan-Details angezeigt.</p>;
  return <fieldset className="grid gap-3 border border-[var(--line)] bg-[var(--paper-muted)] p-4"><legend className="px-1 text-xs font-bold text-[var(--ink)]">Optionale Ausführungsbedingung</legend><label className="flex cursor-pointer items-center gap-3 text-sm font-semibold text-[var(--ink)]"><input className="!h-4 !w-4 !shrink-0 accent-lxcup-primary" aria-label="Schwellwert aktivieren" type="checkbox" checked={thresholdEnabled} onChange={(event) => onThresholdEnabled(event.target.checked)} /><span>Nur bei erreichtem Schwellwert starten</span></label><ThresholdFields enabled={thresholdEnabled} metric={metric} onMetricChange={onMetricChange} operator={operator} onOperatorChange={onOperatorChange} value={value} onValueChange={onValueChange} /></fieldset>;
}

function ThresholdFields({ enabled, metric, onMetricChange, operator, onOperatorChange, value, onValueChange }: Readonly<{ enabled: boolean; metric: ThresholdMetric; onMetricChange: (metric: ThresholdMetric) => void; operator: "greater_than_or_equal" | "less_than_or_equal"; onOperatorChange: (operator: "greater_than_or_equal" | "less_than_or_equal") => void; value: string; onValueChange: (value: string) => void }>) {
  if (!enabled) return <p className="m-0 text-xs text-[var(--muted)]">Ohne Schwellwert läuft der Job ausschließlich nach dem gewählten Intervall.</p>;
  return <div className="grid gap-3 sm:grid-cols-3"><label className={fieldClass}><span>Metrik</span><select className={inputClass} value={metric} onChange={(event) => onMetricChange(event.target.value as typeof metric)}><option value="cpu_basis_points">CPU-Auslastung</option><option value="memory_basis_points">RAM-Auslastung</option><option value="storage_basis_points">Speicherauslastung</option><option value="heartbeat_age_seconds">Heartbeat-Alter</option></select></label><label className={fieldClass}><span>Bedingung</span><select className={inputClass} value={operator} onChange={(event) => onOperatorChange(event.target.value as typeof operator)}><option value="greater_than_or_equal">größer oder gleich</option><option value="less_than_or_equal">kleiner oder gleich</option></select></label><label className={fieldClass}><span>Grenzwert</span><input className={inputClass} type="number" min="0" value={value} onChange={(event) => onValueChange(event.target.value)} /></label></div>;
}

function scheduleAvailabilityLabel(isBackup: boolean, targetCount: number) {
  if (isBackup) return "Backups sind nur für Admins planbar.";
  if (targetCount > 0) return `${targetCount} Ziele verfügbar`;
  return "Lege zuerst ein Ziel an, um einen Zeitplan zu erstellen.";
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
        <td>{schedule.operation === "create_backup" ? "Controller · Backup-Secret hinterlegt" : schedule.target_ids.map((targetId) => { const target = targets.find((item) => item.id === targetId); return target ? targetDisplayLabel(target) : targetId; }).join(", ")}</td>
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
    create_backup: "Backup erstellen",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}
