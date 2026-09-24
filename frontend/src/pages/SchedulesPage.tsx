import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createSchedule, createUpdatePolicy } from "../api";
import { queryKeys, useSchedules, useTargets, useUpdatePolicies } from "../queries";
import { cn, ui } from "../ui";

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
  const [policyName, setPolicyName] = useState("");
  const [policyTargetId, setPolicyTargetId] = useState("");
  const [allowedPackages, setAllowedPackages] = useState("");
  const [maximumRisk, setMaximumRisk] = useState<"low" | "medium" | "high">("high");
  const [policyStart, setPolicyStart] = useState("00:00");
  const [policyEnd, setPolicyEnd] = useState("23:59");
  const [policyTimezone, setPolicyTimezone] = useState("UTC");
  const mutation = useMutation({
    mutationFn: () => createSchedule({ id, operation, timezone, target_ids: [targetId], every_minutes: Number(everyMinutes), enabled: true, threshold: thresholdEnabled ? { metric: thresholdMetric, operator: thresholdOperator, value: Number(thresholdValue) } : null, policy_id: operation === "update_packages" ? policyId : null }),
    onSuccess: () => { setId(""); void queryClient.invalidateQueries({ queryKey: queryKeys.schedules }); },
  });
  const policyMutation = useMutation({
    mutationFn: () => createUpdatePolicy({ id: policyName, allowed_targets: [policyTargetId], allowed_packages: allowedPackages.split(",").map((item) => item.trim()).filter(Boolean), maintenance_start_minute: toMinute(policyStart), maintenance_end_minute: toMinute(policyEnd), timezone: policyTimezone, maximum_risk: maximumRisk, enabled: true }),
    onSuccess: () => { setPolicyName(""); setAllowedPackages(""); void queryClient.invalidateQueries({ queryKey: queryKeys.updatePolicies }); },
  });
  const canSubmit = id.trim() !== "" && targetId !== "" && Number(everyMinutes) > 0 && (operation !== "update_packages" || policyId !== "");
  return <>
    <header className={ui.pageHeader}><div><p className={ui.eyebrow}>Automatisierung</p><h1>Zeitpläne</h1><p className={ui.muted}>Wiederkehrende, registrierte Jobs mit Ziel- und Zeitzonenbezug.</p></div></header>
    <section className={ui.panel}><h2>Update-Policy anlegen</h2><p className={ui.muted}>Legt zulässige Ziele, optionale Paketgruppen und maximales Risiko fest. Das Wartungsfenster gilt in UTC.</p><form onSubmit={(event) => { event.preventDefault(); if (policyName.trim() && policyTargetId) policyMutation.mutate(); }}><div className={ui.workflowGrid}>
      <label><span>Name</span><input value={policyName} onChange={(event) => setPolicyName(event.target.value)} placeholder="security-updates" required /></label>
      <label><span>Ziel</span><select value={policyTargetId} onChange={(event) => setPolicyTargetId(event.target.value)} required><option value="">Ziel auswählen</option>{(targets.data ?? []).map((target) => <option key={target.id} value={target.id}>{target.name}</option>)}</select></label>
      <label><span>Erlaubte Pakete (Komma-getrennt, leer = alle)</span><input value={allowedPackages} onChange={(event) => setAllowedPackages(event.target.value)} placeholder="curl, openssl" /></label>
      <label><span>Maximales Risiko</span><select value={maximumRisk} onChange={(event) => setMaximumRisk(event.target.value as typeof maximumRisk)}><option value="low">Niedrig</option><option value="medium">Mittel</option><option value="high">Hoch</option></select></label>
      <label><span>Wartungsfenster von</span><input type="time" value={policyStart} onChange={(event) => setPolicyStart(event.target.value)} /></label>
      <label><span>Wartungsfenster bis</span><input type="time" value={policyEnd} onChange={(event) => setPolicyEnd(event.target.value)} /></label>
      <label><span>Zeitzone</span><input value={policyTimezone} readOnly aria-readonly="true" /></label>
    </div><button className={ui.button} type="submit" disabled={!policyName.trim() || !policyTargetId || policyMutation.isPending}>{policyMutation.isPending ? "Speichert…" : "Policy speichern"}</button>{policyMutation.error ? <p className={ui.errorState} role="alert">{policyMutation.error.message}</p> : null}</form>{policies.data?.length ? <ul className={ui.auditList}>{policies.data.map((policy) => <li key={policy.id}><strong>{policy.id}</strong><span className={ui.muted}>{policy.enabled ? "aktiv" : "deaktiviert"} · Risiko {policy.maximum_risk} · {policy.allowed_packages.join(", ") || "alle Pakete"}</span></li>)}</ul> : <p className={ui.muted}>Noch keine Policies angelegt.</p>}</section>
    <section className={ui.panel}><form onSubmit={(event) => { event.preventDefault(); if (canSubmit) mutation.mutate(); }}><div className={ui.workflowGrid}>
      <label><span>Name</span><input value={id} onChange={(event) => setId(event.target.value)} placeholder="nightly-inventory" /></label>
      <label><span>Ziel</span><select value={targetId} onChange={(event) => setTargetId(event.target.value)}><option value="">Ziel auswählen</option>{(targets.data ?? []).map((target) => <option key={target.id} value={target.id}>{target.name}</option>)}</select></label>
      <label><span>Job</span><select value={operation} onChange={(event) => setOperation(event.target.value)}><option value="collect_package_inventory">Paketinventar</option><option value="health_check">Healthcheck</option><option value="update_packages">Paketupdate</option></select></label>
      {operation === "update_packages" ? <label><span>Update-Policy</span><select value={policyId} onChange={(event) => setPolicyId(event.target.value)}><option value="">Policy auswählen</option>{(policies.data ?? []).filter((policy) => policy.enabled).map((policy) => <option key={policy.id} value={policy.id}>{policy.id}</option>)}</select></label> : null}
      <label><span>Intervall (Minuten)</span><input type="number" min="1" max="10080" value={everyMinutes} onChange={(event) => setEveryMinutes(event.target.value)} /></label>
      <label><span>Zeitzone</span><input value={timezone} onChange={(event) => setTimezone(event.target.value)} /></label>
      <label className="flex items-center gap-2"><input aria-label="Schwellwert aktivieren" type="checkbox" checked={thresholdEnabled} onChange={(event) => setThresholdEnabled(event.target.checked)} /><span>Schwellwert aktivieren</span></label>
      {thresholdEnabled ? <>
        <label><span>Metrik</span><select value={thresholdMetric} onChange={(event) => setThresholdMetric(event.target.value as typeof thresholdMetric)}><option value="cpu_basis_points">CPU (Basispunkte)</option><option value="memory_basis_points">RAM (Basispunkte)</option><option value="storage_basis_points">Speicher (Basispunkte)</option><option value="heartbeat_age_seconds">Heartbeat-Alter (Sekunden)</option></select></label>
        <label><span>Operator</span><select value={thresholdOperator} onChange={(event) => setThresholdOperator(event.target.value as typeof thresholdOperator)}><option value="greater_than_or_equal">größer/gleich</option><option value="less_than_or_equal">kleiner/gleich</option></select></label>
        <label><span>Grenzwert</span><input type="number" min="0" value={thresholdValue} onChange={(event) => setThresholdValue(event.target.value)} /></label>
      </> : null}
    </div><button className={ui.primaryButton} type="submit" disabled={!canSubmit || mutation.isPending}>{mutation.isPending ? "Speichert…" : "Zeitplan anlegen"}</button>{mutation.error ? <p className={ui.errorState} role="alert">{mutation.error.message}</p> : null}</form></section>
    <section className={ui.panel}><div className={ui.sectionHeading}><div><h2>Aktive Zeitpläne</h2><p className={ui.muted}>Doppelstarts und Ziel-Exklusivität werden vor dem Einreihen geprüft.</p></div><span className={ui.muted}>{schedules.data?.length ?? 0}</span></div>{schedules.isLoading ? <p className={ui.muted}>Zeitpläne werden geladen…</p> : schedules.error ? <p className={ui.errorState} role="alert">{schedules.error.message}</p> : schedules.data?.length ? <div className={ui.tableWrap}><table><thead><tr><th>Name</th><th>Job</th><th>Zeitzone</th><th>Letzter Lauf</th><th>Nächster Lauf</th><th>Fehler</th><th>Status</th></tr></thead><tbody>{schedules.data.map((schedule) => <tr key={schedule.id}><td>{schedule.id}</td><td>{schedule.operation.replaceAll("_", " ")}</td><td>{schedule.timezone}</td><td>{schedule.last_run_at ? new Date(schedule.last_run_at).toLocaleString() : "Noch nicht ausgeführt"}</td><td>{new Date(schedule.next_run_at).toLocaleString()}</td><td>{schedule.last_error ?? "—"}</td><td><span className={cn(ui.statusBadge, schedule.enabled ? ui.statusSuccess : ui.statusNeutral)}>{schedule.enabled ? "aktiv" : "pausiert"}</span></td></tr>)}</tbody></table></div> : <p className={ui.emptyState}>Noch keine Zeitpläne angelegt.</p>}</section>
  </>;
}

function toMinute(time: string) {
  const [hours, minutes] = time.split(":").map(Number);
  return hours * 60 + minutes;
}
