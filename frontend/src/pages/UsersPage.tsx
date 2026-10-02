import { useState, type SubmitEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, createUser, deleteUser, getSupportDiagnostics, listUsers, resetUserPassword, updateUser, type ManagedUserDto } from "../api";
import { queryKeys } from "../queries";
import { PasswordInput } from "../components/PasswordInput";

export default function UsersPage() {
  const queryClient = useQueryClient();
  const users = useQuery({ queryKey: queryKeys.users, queryFn: ({ signal }) => listUsers(signal) });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<"user" | "admin">("user");
  const [validationError, setValidationError] = useState<string | null>(null);
  const [diagnosticsPending, setDiagnosticsPending] = useState(false);
  const [diagnosticsError, setDiagnosticsError] = useState<string | null>(null);
  const [diagnosticsDays, setDiagnosticsDays] = useState(7);
  const create = useMutation({
    mutationFn: () => createUser({ username, password, role }),
    onSuccess: async () => { setUsername(""); setPassword(""); setValidationError(null); await queryClient.invalidateQueries({ queryKey: queryKeys.users }); },
  });
  const update = useMutation({ mutationFn: ({ user, patch }: { user: ManagedUserDto; patch: { role: "admin" | "user"; disabled: boolean } }) => updateUser(user.id, patch), onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.users }) });
  const remove = useMutation({ mutationFn: (user: ManagedUserDto) => deleteUser(user.id), onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.users }) });
  const reset = useMutation({ mutationFn: ({ user, password }: { user: ManagedUserDto; password: string }) => resetUserPassword(user.id, password), onSuccess: () => queryClient.invalidateQueries({ queryKey: queryKeys.users }) });
  const submit = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!/^[A-Za-z0-9._-]+$/.test(username)) { setValidationError("Der Benutzername darf nur Buchstaben, Zahlen, Punkt, Bindestrich und Unterstrich enthalten."); return; }
    if (password.length < 12) { setValidationError("Das Startpasswort muss mindestens 12 Zeichen enthalten."); return; }
    setValidationError(null);
    create.mutate();
  };
  const downloadDiagnostics = async () => {
    setDiagnosticsPending(true);
    setDiagnosticsError(null);
    try {
      const diagnostics = await getSupportDiagnostics(diagnosticsDays);
      const blob = new Blob([`${JSON.stringify(diagnostics, null, 2)}\n`], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `lxcup-support-${new Date().toISOString().replaceAll(":", "-")}.json`;
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      setDiagnosticsError(errorMessage(error, "Diagnosepaket konnte nicht erstellt werden."));
    } finally {
      setDiagnosticsPending(false);
    }
  };

  return <div className="grid gap-4">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4">
      <div className="min-w-0"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Administration</p><h1 className="mb-1">Benutzerverwaltung</h1><p className="mb-0 text-sm text-[var(--muted)]">Verwalte lokale Konten und ihre Berechtigungen. Benutzer benötigen keine E-Mail-Adresse.</p></div>
      <div className="flex max-w-full flex-wrap items-end gap-2 border border-[var(--line)] bg-[var(--panel)] p-2">
        <div className="flex h-10 min-w-24 items-center gap-2 border-r border-[var(--line)] px-3" aria-label={`${users.data?.length ?? 0} Konten`}>
          <strong className="text-base tabular-nums text-[var(--ink)]">{users.data?.length ?? 0}</strong>
          <span className="text-xs text-[var(--muted)]">{users.data?.length === 1 ? "Konto" : "Konten"}</span>
        </div>
        <label className="grid gap-1 text-[10px] font-semibold text-[var(--muted)]">Zeitraum<select className="h-10 min-w-36 text-xs" value={diagnosticsDays} onChange={(event) => setDiagnosticsDays(Number(event.target.value))}><option value={1}>Letzter Tag</option><option value={7}>Letzte 7 Tage</option><option value={30}>Letzte 30 Tage</option></select></label>
        <button className="inline-flex h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-3 text-xs font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-wait disabled:opacity-50" type="button" disabled={diagnosticsPending} onClick={() => void downloadDiagnostics()}>
          <svg aria-hidden="true" className="h-4 w-4 shrink-0" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8"><path strokeLinecap="round" strokeLinejoin="round" d="M12 3v12m0 0 4-4m-4 4-4-4M4 17v3h16v-3" /></svg>
          {diagnosticsPending ? "Wird erstellt …" : "Support-Diagnose herunterladen"}
        </button>
      </div>
    </header>
    <p className="m-0 text-xs text-[var(--muted)]">Enthält bis zu 250 Ressourcen und 50 Workflows aus dem gewählten Zeitraum. Das Diagnosepaket wird lokal geladen; Zugangsdaten und ungefilterte Worker-Ausgaben sind ausgeschlossen. Es erfolgt kein Upload.</p>
    {diagnosticsError ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{diagnosticsError}</p> : null}

    <section className="grid gap-5 border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)]" aria-labelledby="user-create-heading">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-[var(--line)] pb-4">
        <div><h2 className="mb-1" id="user-create-heading">Benutzer anlegen</h2><p className="mb-0 text-sm text-[var(--muted)]">Das Startpasswort muss beim ersten Login geändert werden.</p></div>
        <span className="border border-[var(--line)] bg-[var(--paper-muted)] px-2.5 py-1 text-[11px] font-medium text-[var(--muted)]">Kein E-Mail-Konto erforderlich</span>
      </div>
      <form className="grid items-start gap-x-4 gap-y-4 md:grid-cols-2 xl:grid-cols-[minmax(14rem,1fr)_minmax(16rem,1.15fr)_minmax(10rem,0.7fr)_auto]" noValidate onSubmit={submit}>
        <label className="grid min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]">Benutzername<input className="h-10" autoComplete="off" maxLength={64} pattern="[A-Za-z0-9._-]+" value={username} onChange={(event) => { setUsername(event.target.value); setValidationError(null); }} required /></label>
        <label className="grid min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]">Startpasswort<PasswordInput className="h-10" autoComplete="new-password" minLength={12} value={password} onChange={(event) => { setPassword(event.target.value); setValidationError(null); }} required /><small className="font-normal text-[var(--muted)]">Mindestens 12 Zeichen</small></label>
        <label className="grid min-w-0 gap-1.5 text-xs font-semibold text-[var(--muted)]">Rolle<select className="h-10" value={role} onChange={(event) => setRole(event.target.value as "user" | "admin")}><option value="user">User</option><option value="admin">Admin</option></select></label>
         <button className="inline-flex min-h-10 items-center justify-center gap-2 self-start border border-lxcup-primary bg-lxcup-primary px-4 text-xs font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-wait disabled:opacity-50 xl:mt-5" type="submit" disabled={create.isPending}>{create.isPending ? "Wird erstellt …" : "Benutzer erstellen"}<span className="text-base leading-none" aria-hidden="true">→</span></button>
      </form>
      {validationError ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{validationError}</p> : null}
      {create.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{errorMessage(create.error, "Benutzer konnte nicht erstellt werden.")}</p> : null}
    </section>

    <section className="grid gap-4 border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)]" aria-labelledby="user-list-heading">
      <div className="flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-4"><div><h2 className="mb-1" id="user-list-heading">Konten</h2><p className="mb-0 text-sm text-[var(--muted)]">Deaktivierte Benutzer können sich nicht anmelden. Rollen- oder Passwortänderungen beenden bestehende Sitzungen.</p></div><span className="text-xs text-[var(--muted)]">{users.data?.length ?? 0} {users.data?.length === 1 ? "Konto" : "Konten"}</span></div>
      {users.isPending ? <output className="m-0 text-sm text-[var(--muted)]">Benutzer werden geladen …</output> : null}
      {users.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{errorMessage(users.error, "Benutzer konnten nicht geladen werden.")}</p> : null}
      {users.data?.length === 0 ? <p className="m-0 border border-dashed border-[var(--line)] p-5 text-center text-sm text-[var(--muted)]">Noch keine Benutzer vorhanden.</p> : null}
      {users.data && users.data.length > 0 ? <ul className="m-0 grid list-none gap-2 p-0">{users.data.map((user) => <li key={user.id}><UserRow user={user} resetPending={reset.isPending} onUpdate={(patch) => update.mutate({ user, patch })} onDelete={() => { if (globalThis.confirm(`Benutzer „${user.username}“ wirklich löschen?`)) remove.mutate(user); }} onReset={(temporaryPassword) => reset.mutate({ user, password: temporaryPassword })} /></li>)}</ul> : null}
      {update.error || remove.error || reset.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{errorMessage(update.error ?? remove.error ?? reset.error, "Die Benutzeränderung ist fehlgeschlagen.")}</p> : null}
    </section>
  </div>;
}

function UserRow({ user, resetPending, onUpdate, onDelete, onReset }: Readonly<{ user: ManagedUserDto; resetPending: boolean; onUpdate: (patch: { role: "admin" | "user"; disabled: boolean }) => void; onDelete: () => void; onReset: (password: string) => void }>) {
  const [temporaryPassword, setTemporaryPassword] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);
  const submitReset = (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (temporaryPassword.length < 12) { setValidationError("Das Startpasswort muss mindestens 12 Zeichen enthalten."); return; }
    setValidationError(null);
    onReset(temporaryPassword);
    setTemporaryPassword("");
  };
  return <article aria-label={`Benutzerkonto ${user.username}`} className="grid gap-4 border border-[var(--line)] bg-[var(--paper)] p-4 xl:grid-cols-[minmax(13rem,1.15fr)_minmax(10rem,0.8fr)_minmax(8rem,0.65fr)_minmax(28rem,2.2fr)] xl:items-center">
    <div className="flex min-w-0 items-center gap-3">
      <span aria-hidden="true" className="grid h-10 w-10 shrink-0 place-items-center border border-[var(--line)] bg-[var(--panel)] text-xs font-bold uppercase text-lxcup-primary">{user.username.slice(0, 2)}</span>
      <div className="min-w-0"><strong className="block truncate text-sm">{user.username}</strong>{user.must_change_password ? <small className="mt-1 inline-flex border border-[var(--warning)]/40 bg-[var(--warning-soft)] px-2 py-0.5 text-[10px] font-semibold text-[var(--warning)]">Passwortwechsel ausstehend</small> : <small className="mt-1 block text-xs text-[var(--muted)]">Lokales Konto</small>}</div>
    </div>
      <label className="grid min-w-0 gap-1 text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Rolle<select className="text-sm font-normal normal-case tracking-normal" aria-label={`Rolle für ${user.username}`} value={user.role} onChange={(event) => onUpdate({ role: event.target.value as "admin" | "user", disabled: user.disabled })}><option value="user">User</option><option value="admin">Admin</option></select></label>
    <div className="grid content-start gap-1"><span className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Status</span><span className={`inline-flex w-fit items-center gap-2 border px-2 py-1 text-xs font-semibold ${user.disabled ? "border-[var(--line)] bg-[var(--paper-muted)] text-[var(--muted)]" : "border-[var(--success)]/30 bg-[var(--success-soft)] text-[var(--success)]"}`}><span aria-hidden="true" className="h-1.5 w-1.5 rounded-full bg-current" />{user.disabled ? "Deaktiviert" : "Aktiv"}</span></div>
    <div className="grid min-w-0 gap-2 border-t border-[var(--line)] pt-3 xl:border-l xl:border-t-0 xl:pl-4 xl:pt-0">
      <span className="text-[10px] font-bold uppercase tracking-wider text-[var(--muted)]">Aktionen</span>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] disabled:opacity-50" type="button" onClick={() => onUpdate({ role: user.role, disabled: !user.disabled })}>{user.disabled ? "Aktivieren" : "Deaktivieren"}</button>
        <form className="grid min-w-[17rem] flex-1 grid-cols-[minmax(0,1fr)_auto] items-center gap-2" noValidate onSubmit={submitReset}><PasswordInput aria-label={`Temporäres Passwort für ${user.username}`} autoComplete="new-password" minLength={12} placeholder="Neues Startpasswort" value={temporaryPassword} onChange={(event) => { setTemporaryPassword(event.target.value); setValidationError(null); }} required /><button className="inline-flex min-h-9 shrink-0 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] disabled:cursor-wait disabled:opacity-50" type="submit" disabled={resetPending}>Zurücksetzen</button>{validationError ? <span role="alert" className="col-span-2 text-xs text-[var(--error)]">{validationError}</span> : null}</form>
        <button className="inline-flex min-h-9 items-center justify-center border border-[var(--error)]/50 bg-[var(--panel)] px-3 text-xs font-semibold text-[var(--error)] hover:border-[var(--error)] hover:bg-[var(--error-soft)]" type="button" onClick={onDelete}>Löschen</button>
      </div>
    </div>
  </article>;
}

function errorMessage(error: unknown, fallback: string) {
  return error instanceof ApiError ? error.message : fallback;
}
