import { useState, type SubmitEvent } from "react";
import { ApiError, changePassword } from "../api";
import { PasswordInput } from "../components/PasswordInput";

export default function AccountPasswordPage() {
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [pending, setPending] = useState(false);
  const submit = async (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    setSaved(false);
    if (currentPassword.length === 0) { setError("Bitte dein aktuelles Passwort eingeben."); return; }
    if (newPassword.length < 12) { setError("Das neue Passwort muss mindestens 12 Zeichen enthalten."); return; }
    if (confirmation.length === 0) { setError("Bitte das neue Passwort wiederholen."); return; }
    if (newPassword !== confirmation) { setError("Die neuen Passwörter stimmen nicht überein."); return; }
    setPending(true);
    setError(null);
    try {
      await changePassword(newPassword, currentPassword);
      setCurrentPassword(""); setNewPassword(""); setConfirmation(""); setSaved(true);
    } catch (error_) {
      setError(error_ instanceof ApiError ? error_.message : "Das Passwort konnte nicht geändert werden.");
    } finally { setPending(false); }
  };
  return <div className="grid gap-4">
    <header className="border-b border-[var(--line)] pb-4"><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Mein Konto</p><h1 className="mb-1">Passwort ändern</h1><p className="mb-0 text-sm text-[var(--muted)]">Ändere dein Passwort. Aus Sicherheitsgründen werden andere aktive Sitzungen danach beendet.</p></header>
    <section className="max-w-2xl border border-[var(--line)] bg-[var(--panel)] p-5 text-[var(--ink)]">
      <form className="grid gap-4" noValidate onSubmit={(event) => void submit(event)}>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Aktuelles Passwort<PasswordInput autoComplete="current-password" value={currentPassword} onChange={(event) => { setCurrentPassword(event.target.value); setError(null); }} required /></label>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Neues Passwort<PasswordInput autoComplete="new-password" minLength={12} value={newPassword} onChange={(event) => { setNewPassword(event.target.value); setError(null); }} required /><small className="font-normal">Mindestens 12 Zeichen</small></label>
        <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Neues Passwort wiederholen<PasswordInput autoComplete="new-password" minLength={12} value={confirmation} onChange={(event) => { setConfirmation(event.target.value); setError(null); }} required /></label>
        {error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{error}</p> : null}
        {saved ? <output className="m-0 text-sm text-[var(--success)]">Dein Passwort wurde geändert.</output> : null}
        <button className="inline-flex min-h-10 justify-self-start items-center border border-lxcup-primary bg-lxcup-primary px-4 text-xs font-semibold text-white hover:bg-blue-700 disabled:opacity-50" type="submit" disabled={pending}>{pending ? "Wird gespeichert …" : "Passwort speichern"}</button>
      </form>
    </section>
  </div>;
}
