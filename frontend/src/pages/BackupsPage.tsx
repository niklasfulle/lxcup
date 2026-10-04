import { useState, type SubmitEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, createAdminBackup, downloadAdminBackup, listAdminBackups, type AdminBackupDto } from "../api";
import { queryKeys } from "../queries";

export function BackupsPage() {
  const queryClient = useQueryClient();
  const backups = useQuery({ queryKey: queryKeys.adminBackups, queryFn: ({ signal }) => listAdminBackups(signal) });
  const [downloadId, setDownloadId] = useState<string | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [createError, setCreateError] = useState<unknown>(null);
  const [confirmationOpen, setConfirmationOpen] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [passphraseConfirmation, setPassphraseConfirmation] = useState("");
  const [creating, setCreating] = useState(false);
  const passphrasesMatch = passphrase === passphraseConfirmation;
  const passphraseValid = Array.from(passphrase).length >= 12 && new TextEncoder().encode(passphrase).length <= 1024;

  const download = async (backup: AdminBackupDto) => {
    setDownloadId(backup.id);
    setDownloadError(null);
    try {
      const blob = await downloadAdminBackup(backup.id);
      const url = URL.createObjectURL(blob);
      const revokeObjectUrl = typeof URL.revokeObjectURL === "function"
        ? URL.revokeObjectURL.bind(URL)
        : undefined;
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `lxcup-backup-${backup.id}.tar.age`;
      anchor.click();
      if (revokeObjectUrl) globalThis.setTimeout(() => revokeObjectUrl(url), 1000);
    } catch (error) {
      setDownloadError(message(error, "Backup konnte nicht geladen werden."));
    } finally {
      setDownloadId(null);
    }
  };

  const createBackup = async (event: SubmitEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!passphraseValid || !passphrasesMatch || creating) return;
    const secret = passphrase;
    setPassphrase("");
    setPassphraseConfirmation("");
    setConfirmationOpen(false);
    setCreateError(null);
    setCreating(true);
    try {
      await createAdminBackup(secret);
      void queryClient.invalidateQueries({ queryKey: queryKeys.adminBackups });
    } catch (error) {
      setCreateError(error);
    } finally {
      setCreating(false);
    }
  };

  const closeConfirmation = () => {
    setConfirmationOpen(false);
    setPassphrase("");
    setPassphraseConfirmation("");
  };

  return <div className="grid gap-4">
    <header className="flex flex-wrap items-end justify-between gap-4 border-b border-[var(--line)] pb-4">
      <div><p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Administration · Wiederherstellung</p><h1 className="mb-1">Backups</h1><p className="mb-0 text-sm text-[var(--muted)]">Erstelle ein verschlüsseltes Backup der Datenbank und des verschlüsselten Secret-Stores.</p></div>
      <div className="relative">
        <button className="inline-flex min-h-10 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-4 text-xs font-semibold text-white transition-colors hover:bg-blue-700 disabled:cursor-wait disabled:opacity-50" type="button" aria-expanded={confirmationOpen} aria-controls="backup-confirmation" onClick={() => confirmationOpen ? closeConfirmation() : setConfirmationOpen(true)} disabled={creating}>{creating ? "Backup wird erstellt …" : "Backup jetzt erstellen"}</button>
        {confirmationOpen ? <BackupConfirmationDialog
          passphrase={passphrase}
          passphraseConfirmation={passphraseConfirmation}
          passphrasesMatch={passphrasesMatch}
          passphraseValid={passphraseValid}
          onPassphraseChange={setPassphrase}
          onPassphraseConfirmationChange={setPassphraseConfirmation}
          onSubmit={createBackup}
          onClose={closeConfirmation}
        /> : null}
      </div>
    </header>

    <section className="grid gap-2 border border-[var(--primary)]/30 bg-[var(--primary-soft)] p-4 text-sm text-[var(--ink)]" aria-label="Backup-Sicherheitshinweise">
      <strong>Verschlüsselung und Passwort</strong>
      <p className="m-0 text-[var(--muted)]">Kein externer Upload: Das verschlüsselte Archiv wird nur im persistenten Backup-Volume dieses Servers gespeichert. Nach dem Erstellen erscheint es unten in der Liste und wird erst durch deinen Klick auf „Herunterladen“ an deinen Browser übertragen.</p>
      <p className="m-0 text-[var(--muted)]">Jedes Backup wird mit dem beim Erstellen gewählten Passwort im age-Format verschlüsselt. Dieses Passwort wird nicht auf dem Server gespeichert und kann nicht zurückgesetzt werden. Bewahre es sicher auf.</p>
      <p className="m-0 text-[var(--muted)]">Der Secret-Master-Key und deine Live-<code>.env</code> sind nicht enthalten. Die versionierten Compose-Dateien und <code>.env.example</code> liegen nur als Vorlage bei. Für eine Wiederherstellung brauchst du sowohl das Backup-Passwort als auch <code>LXCUP_SECRET_MASTER_KEY</code>. Das Backup enthält sensible System- und Kontodaten.</p>
    </section>

    {createError ? <div role="alert" className="grid gap-1 border border-[var(--error)]/30 bg-[var(--error-soft)] p-3 text-sm text-[var(--error)]"><strong>{message(createError, "Backup konnte nicht erstellt werden.")}</strong><span>{failureHint(createError)}</span></div> : null}
    {downloadError ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{downloadError}</p> : null}
    <section className="grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]" aria-labelledby="backup-list-title">
      <div className="flex flex-wrap items-end justify-between gap-3 border-b border-[var(--line)] pb-3"><div><h2 className="mb-1" id="backup-list-title">Gespeicherte Backups</h2><p className="mb-0 text-sm text-[var(--muted)]">Hier bleiben sie bis zum Ablauf der Aufbewahrungszeit. Der Download startet nur manuell über „Herunterladen“.</p></div><span className="text-xs text-[var(--muted)]">{backups.data?.length ?? 0} Einträge</span></div>
      {backups.isPending ? <p className="m-0 text-sm text-[var(--muted)]">Backups werden geladen …</p> : null}
      {backups.error ? <p role="alert" className="m-0 text-sm text-[var(--error)]">{message(backups.error, "Backups konnten nicht geladen werden.")}</p> : null}
      {backups.data?.length === 0 ? <p className="m-0 border border-dashed border-[var(--line)] p-5 text-center text-sm text-[var(--muted)]">Noch keine Backups erstellt.</p> : null}
      {backups.data && backups.data.length > 0 ? <ul className="m-0 grid list-none gap-2 p-0">{backups.data.map((backup) => <li key={backup.id} className="flex flex-wrap items-center justify-between gap-3 border border-[var(--line)] bg-[var(--paper)] p-3"><div className="min-w-0"><strong className="block truncate text-sm">{new Date(backup.created_at).toLocaleString()}</strong><span className="text-xs text-[var(--muted)]">{formatBytes(backup.size_bytes)} · verschlüsselt · {backup.id}</span></div><button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 text-xs font-semibold hover:border-[var(--primary)] hover:bg-[var(--primary-soft)] disabled:cursor-wait disabled:opacity-50" type="button" disabled={downloadId === backup.id} onClick={() => void download(backup)}>{downloadId === backup.id ? "Wird geladen …" : "Herunterladen"}</button></li>)}</ul> : null}
    </section>
  </div>;
}

function BackupConfirmationDialog({
  passphrase,
  passphraseConfirmation,
  passphrasesMatch,
  passphraseValid,
  onPassphraseChange,
  onPassphraseConfirmationChange,
  onSubmit,
  onClose,
}: Readonly<{
  passphrase: string;
  passphraseConfirmation: string;
  passphrasesMatch: boolean;
  passphraseValid: boolean;
  onPassphraseChange: (value: string) => void;
  onPassphraseConfirmationChange: (value: string) => void;
  onSubmit: (event: SubmitEvent<HTMLFormElement>) => void;
  onClose: () => void;
}>) {
  return <dialog open aria-labelledby="backup-confirmation-title" className="absolute right-0 top-full z-20 mt-2 w-[min(26rem,calc(100vw-2rem))] border border-[var(--line)] bg-[var(--panel)] p-0 text-left text-sm text-[var(--ink)] shadow-xl">
    <form id="backup-confirmation" onSubmit={onSubmit} className="grid gap-3 p-4">
      <div><h2 id="backup-confirmation-title" className="mb-1 text-sm">Passwortgeschütztes Backup erstellen?</h2><p className="m-0 text-[var(--muted)]">Datenbank, verschlüsselte Secrets und Konfigurationsvorlagen werden archiviert. Das Backup bleibt auf diesem Server und wird nicht automatisch heruntergeladen oder extern übertragen.</p></div>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Backup-Passwort<input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={passphrase} onChange={(event) => onPassphraseChange(event.target.value)} className="h-10" aria-describedby="backup-passphrase-help" /></label>
      <label className="grid gap-1.5 text-xs font-semibold text-[var(--muted)]">Passwort wiederholen<input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={passphraseConfirmation} onChange={(event) => onPassphraseConfirmationChange(event.target.value)} className="h-10" /></label>
      <p id="backup-passphrase-help" className="m-0 text-xs text-[var(--muted)]">Mindestens 12 Zeichen. Das Passwort wird nicht gespeichert. Bewahre es sicher auf – ohne Passwort ist das Backup nicht wiederherstellbar.</p>
      {passphraseConfirmation && !passphrasesMatch ? <p role="alert" className="m-0 text-xs text-[var(--error)]">Die Passwörter stimmen nicht überein.</p> : null}
      {!passphraseValid && passphrase ? <p role="alert" className="m-0 text-xs text-[var(--error)]">Das Passwort muss mindestens 12 Zeichen lang sein.</p> : null}
      <div className="flex justify-end gap-2"><button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] px-3 text-xs font-semibold hover:bg-[var(--paper-muted)]" type="button" onClick={onClose}>Abbrechen</button><button className="inline-flex min-h-9 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 text-xs font-semibold text-white hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50" type="submit" disabled={!passphraseValid || !passphrasesMatch}>Verschlüsseltes Backup erstellen</button></div>
    </form>
  </dialog>;
}

function formatBytes(value: number) {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let size = value;
  let unit = -1;
  do { size /= 1024; unit += 1; } while (size >= 1024 && unit < units.length - 1);
  return `${size.toFixed(1)} ${units[unit]}`;
}

function message(error: unknown, fallback: string) {
  return error instanceof ApiError ? error.message : fallback;
}

function failureHint(error: unknown) {
  if (error instanceof ApiError && error.code === "backup_database_failed") {
    return "Der PostgreSQL-Dump ist fehlgeschlagen. Prüfe Datenbankzugriff und pg_dump im Server-Container.";
  }
  if (error instanceof ApiError && error.code === "backup_archive_failed") {
    return "Das Archiv konnte nicht erstellt werden. Prüfe tar sowie Schreibrechte und freien Speicher im Backup-Volume.";
  }
  if (error instanceof ApiError && error.code === "backup_encryption_failed") {
    return "Die Passwortverschlüsselung ist fehlgeschlagen. Prüfe Serverprotokoll und verfügbaren Speicher.";
  }
  return "Prüfe Serverprotokoll, Backup-Volume, Secret-Store und Datenbankzugriff.";
}
