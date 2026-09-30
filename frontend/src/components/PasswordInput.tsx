import { useState, type InputHTMLAttributes } from "react";

type PasswordInputProps = Readonly<Omit<InputHTMLAttributes<HTMLInputElement>, "type">>;

export function PasswordInput({ className = "", ...inputProps }: PasswordInputProps) {
  const [visible, setVisible] = useState(false);
  const visibilityLabel = visible ? "Passwort verbergen" : "Passwort anzeigen";

  return <span className="flex min-w-0">
    <input {...inputProps} className={`min-w-0 flex-1 ${className}`} type={visible ? "text" : "password"} />
    <button
      aria-label={visibilityLabel}
      aria-pressed={visible}
      className="shrink-0 border border-l-0 border-[var(--line)] bg-[var(--paper-muted)] px-3 text-xs font-semibold text-[var(--muted)] hover:text-[var(--ink)]"
      onClick={() => setVisible((current) => !current)}
      title={visibilityLabel}
      type="button"
    >
      <svg aria-hidden="true" className="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
        {visible ? <>
          <path strokeLinecap="round" strokeLinejoin="round" d="M3 3l18 18M10.6 10.6a2 2 0 002.8 2.8" />
          <path strokeLinecap="round" strokeLinejoin="round" d="M9.9 5.2A10.8 10.8 0 0112 5c5 0 8.6 4.3 9.5 6-.4.8-1.2 1.8-2.2 2.8M6.2 6.2C4.3 7.4 3 9.2 2.5 11c.9 1.7 4.5 6 9.5 6 1.2 0 2.3-.2 3.3-.7" />
        </> : <>
          <path strokeLinecap="round" strokeLinejoin="round" d="M2.5 12S6 5 12 5s9.5 7 9.5 7-3.5 7-9.5 7-9.5-7-9.5-7z" />
          <circle cx="12" cy="12" r="3" />
        </>}
      </svg>
    </button>
  </span>;
}
