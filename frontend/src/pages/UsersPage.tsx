import { useState, type SubmitEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, createUser, deleteUser, listUsers, resetUserPassword, updateUser, type ManagedUserDto } from "../api";
import { queryKeys } from "../queries";
import { PasswordInput } from "../components/PasswordInput";

export default function UsersPage() {
  const queryClient = useQueryClient();
  const users = useQuery({ queryKey: queryKeys.users, queryFn: ({ signal }) => listUsers(signal) });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<"user" | "admin">("user");
  const [validationError, setValidationError] = useState<string | null>(null);
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

  return <div className="grid gap-4">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4">
      <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Administration</p><h1 className="mb-1">Benutzerverwaltung</h1><p className="mb-0 text-sm text-[var(--muted)]">Verwalte lokale Konten und ihre Berechtigungen. Benutzer benötigen keine E-Mail-Adresse.</p></div>
      <span className="border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs text-[var(--muted)]"><strong className="text-[var(--ink)]">{users.data?.length ?? 0}</strong> Konten</span>
    </header>

    <section className="grid gap-4 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-labelledby="user-create-heading">
      <div><h2 className="mb-1" id="user-create-heading">Benutzer anlegen</h2><p className="mb-0 text-sm text-[var(--muted)]">Das Startpasswort muss beim ersten Login geändert werden.</p></div>
      <form className="grid items-end gap-3 md:grid-cols-2 xl:grid-cols-[1.2fr_1.2fr_0.8fr_auto]" noValidate onSubmit={submit}>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Benutzername<input autoComplete="off" maxLength={64} pattern="[A-Za-z0-9._-]+" value={username} onChange={(event) => { setUsername(event.target.value); setValidationError(null); }} required /></label>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Startpasswort<PasswordInput autoComplete="new-password" minLength={12} value={password} onChange={(event) => { setPassword(event.target.value); setValidationError(null); }} required /><small className="font-normal">Mindestens 12 Zeichen</small></label>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Rolle<select value={role} onChange={(event) => setRole(event.target.value as "user" | "admin")}><option value="user">User</option><option value="admin">Admin</option></select></label>
        <button className="inline-flex min-h-10 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-4 text-xs font-semibold text-white hover:bg-blue-700 disabled:opacity-50" type="submit" disabled={create.isPending}>{create.isPending ? "Wird erstellt …" : "Benutzer erstellen"}</button>
      </form>
      {validationError ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{validationError}</p> : null}
      {create.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{errorMessage(create.error, "Benutzer konnte nicht erstellt werden.")}</p> : null}
    </section>

    <section className="grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-labelledby="user-list-heading">
      <div><h2 className="mb-1" id="user-list-heading">Konten</h2><p className="mb-0 text-sm text-[var(--muted)]">Deaktivierte Benutzer können sich nicht anmelden. Rollen- oder Passwortänderungen beenden bestehende Sitzungen.</p></div>
      {users.isPending ? <output className="m-0 text-sm text-[var(--muted)]">Benutzer werden geladen …</output> : null}
      {users.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{errorMessage(users.error, "Benutzer konnten nicht geladen werden.")}</p> : null}
      {users.data?.length === 0 ? <p className="m-0 border border-dashed border-[var(--line)] p-5 text-center text-sm text-[var(--muted)]">Noch keine Benutzer vorhanden.</p> : null}
      {users.data && users.data.length > 0 ? <div className="overflow-x-auto"><table><thead><tr><th>Benutzer</th><th>Rolle</th><th>Status</th><th>Aktionen</th></tr></thead><tbody>{users.data.map((user) => <UserRow key={user.id} user={user} resetPending={reset.isPending} onUpdate={(patch) => update.mutate({ user, patch })} onDelete={() => { if (globalThis.confirm(`Benutzer „${user.username}“ wirklich löschen?`)) remove.mutate(user); }} onReset={(temporaryPassword) => reset.mutate({ user, password: temporaryPassword })} />)}</tbody></table></div> : null}
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
  return <tr>
    <td><strong>{user.username}</strong>{user.must_change_password ? <small className="mt-1 block text-[var(--warning)]">Passwortwechsel ausstehend</small> : null}</td>
    <td><select aria-label={`Rolle für ${user.username}`} value={user.role} onChange={(event) => onUpdate({ role: event.target.value as "admin" | "user", disabled: user.disabled })}><option value="user">User</option><option value="admin">Admin</option></select></td>
    <td><span className={user.disabled ? "text-[var(--muted)]" : "text-[var(--success)]"}>{user.disabled ? "Deaktiviert" : "Aktiv"}</span></td>
    <td><div className="flex min-w-[21rem] flex-wrap items-center gap-2">
      <button className="border border-[var(--line)] px-2 py-1 text-xs hover:bg-[var(--primary-soft)]" type="button" onClick={() => onUpdate({ role: user.role, disabled: !user.disabled })}>{user.disabled ? "Aktivieren" : "Deaktivieren"}</button>
      <form className="flex items-center gap-1" noValidate onSubmit={submitReset}><PasswordInput className="max-w-44" aria-label={`Temporäres Passwort für ${user.username}`} autoComplete="new-password" minLength={12} placeholder="Neues Startpasswort" value={temporaryPassword} onChange={(event) => { setTemporaryPassword(event.target.value); setValidationError(null); }} required /><button className="border border-[var(--line)] px-2 py-1 text-xs hover:bg-[var(--primary-soft)] disabled:opacity-50" type="submit" disabled={resetPending}>Zurücksetzen</button>{validationError ? <span role="alert" className="basis-full text-xs text-[var(--error)]">{validationError}</span> : null}</form>
      <button className="border border-[var(--error)] px-2 py-1 text-xs text-[var(--error)] hover:bg-[var(--error-soft)]" type="button" onClick={onDelete}>Löschen</button>
    </div></td>
  </tr>;
}

function errorMessage(error: unknown, fallback: string) {
  return error instanceof ApiError ? error.message : fallback;
}
