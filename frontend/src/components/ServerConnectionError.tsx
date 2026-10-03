type ServerConnectionErrorProps = Readonly<{
  message: string;
  onRetry: () => void;
}>;

export function ServerConnectionError({ message, onRetry }: ServerConnectionErrorProps) {
  return (
    <main className="grid min-h-screen place-items-center bg-[var(--paper)] p-5 text-[var(--ink)]">
      <section className="grid w-full max-w-xl gap-5 border border-[var(--line)] bg-[var(--panel)] p-6 shadow-xl sm:p-8" aria-labelledby="connection-error-title">
        <div className="flex items-center gap-3 border-b border-[var(--line)] pb-4">
          <span className="grid h-10 w-10 shrink-0 place-items-center border border-[var(--error)]/40 bg-[var(--error-soft)] text-[var(--error)]" aria-hidden="true">
            <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth="1.8">
              <rect x="3" y="4" width="18" height="6" rx="1" />
              <rect x="3" y="14" width="18" height="6" rx="1" />
              <path d="M7 7h.01M7 17h.01M12 17h3" strokeLinecap="round" />
              <path d="m18 13 4 4m0-4-4 4" strokeLinecap="round" />
            </svg>
          </span>
          <div>
            <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">lxcup Control Plane</p>
            <p className="m-0 text-xs font-semibold text-[var(--error)]">API-Verbindung fehlgeschlagen</p>
          </div>
        </div>

        <div className="grid gap-2">
          <h1 className="m-0 text-xl font-semibold" id="connection-error-title">Der Server ist gerade nicht erreichbar</h1>
          <p className="m-0 text-sm leading-6 text-[var(--muted)]">lxcup konnte die Anmeldung nicht prüfen. Deine Daten wurden nicht verändert. Sobald der Server wieder erreichbar ist, kannst du es erneut versuchen.</p>
        </div>

        <div className="border border-[var(--error)]/40 bg-[var(--error-soft)] p-3 text-sm text-[var(--error)]" role="alert">
          {message}
        </div>

        <div className="grid gap-2 border border-[var(--line)] bg-[var(--paper-muted)] p-4">
          <h2 className="m-0 text-xs font-bold uppercase tracking-wider text-[var(--muted)]">Das kannst du prüfen</h2>
          <ul className="m-0 grid gap-1 pl-5 text-sm leading-5 text-[var(--muted)]">
            <li>Ist der lxcup-Server gestartet und gesund?</li>
            <li>Leitet ein Reverse Proxy Anfragen an <code className="text-[var(--ink)]">/api</code> korrekt weiter?</li>
            <li>Ist der Server aus diesem Browser erreichbar?</li>
          </ul>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="m-0 text-xs text-[var(--muted)]">Der erneute Versuch prüft die Sitzung direkt beim Server.</p>
          <button className="inline-flex min-h-10 items-center justify-center gap-2 border border-lxcup-primary bg-lxcup-primary px-4 text-sm font-semibold text-white transition-colors hover:bg-blue-700" type="button" onClick={onRetry}>
            <svg viewBox="0 0 20 20" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true">
              <path d="M16.5 7A7 7 0 1 0 17 10" strokeLinecap="round" />
              <path d="M13.5 4.5 16.8 7l-3.8 1.2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
            Erneut verbinden
          </button>
        </div>
      </section>
    </main>
  );
}
