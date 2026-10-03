import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";

type TerminalState = "disconnected" | "connecting" | "connected" | "error";

export function TerminalPanel({ targetId }: Readonly<{ targetId: string }>) {
  const terminalHost = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const [state, setState] = useState<TerminalState>("disconnected");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const host = terminalHost.current;
    if (!host) return;

    const fitAddon = new FitAddon();
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
      fontSize: 13,
      scrollback: 1_000,
      screenReaderMode: true,
      theme: terminalTheme(),
    });
    terminal.loadAddon(fitAddon);
    terminal.open(host);
    terminalRef.current = terminal;
    fitAddonRef.current = fitAddon;
    fitTerminal(fitAddon);

    const send = (message: { type: "input" | "resize"; data?: string; columns?: number; rows?: number }) => {
      const socket = socketRef.current;
      if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
    };
    const inputSubscription = terminal.onData((data) => send({ type: "input", data }));
    const resizeSubscription = terminal.onResize(({ cols, rows }) => send({ type: "resize", columns: cols, rows }));
    const resize = () => fitTerminal(fitAddon);
    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(resize);
    resizeObserver?.observe(host);
    window.addEventListener("resize", resize);

    return () => {
      resizeObserver?.disconnect();
      window.removeEventListener("resize", resize);
      inputSubscription.dispose();
      resizeSubscription.dispose();
      socketRef.current?.close(1000, "terminal panel closed");
      socketRef.current = null;
      terminalRef.current = null;
      fitAddonRef.current = null;
      terminal.dispose();
    };
  }, []);

  const connect = () => {
    if (socketRef.current && socketRef.current.readyState < WebSocket.CLOSING) return;
    setError(null);
    setState("connecting");
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}/api/v1/targets/${encodeURIComponent(targetId)}/terminal`);
    socket.binaryType = "arraybuffer";
    socketRef.current = socket;
    socket.addEventListener("message", (event: MessageEvent<string | ArrayBuffer>) => {
      if (typeof event.data !== "string") {
        terminalRef.current?.write(new Uint8Array(event.data));
        return;
      }
      const payload = parseTerminalMessage(event.data);
      if (payload?.type === "ready") {
        setState("connected");
        const terminal = terminalRef.current;
        terminal?.focus();
        if (terminal && socket.readyState === WebSocket.OPEN) {
          socket.send(JSON.stringify({ type: "resize", columns: terminal.cols, rows: terminal.rows }));
        }
      } else if (payload?.type === "error") {
        setError(payload.message ?? "Die Terminalverbindung ist fehlgeschlagen.");
        setState("error");
        socket.close(1011, "terminal connection failed");
      } else if (payload?.type === "closed") {
        setState("disconnected");
      }
    });
    socket.addEventListener("error", () => {
      setError("Die Terminalverbindung konnte nicht hergestellt werden.");
      setState("error");
    });
    socket.addEventListener("close", () => {
      socketRef.current = null;
      setState((current) => current === "error" ? current : "disconnected");
    });
  };

  const close = () => {
    const socket = socketRef.current;
    if (!socket) return;
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "close" }));
    socket.close(1000, "administrator closed terminal");
    socketRef.current = null;
    setState("disconnected");
  };

  return <section aria-labelledby="terminal-title" className="grid gap-3 border border-[var(--line)] bg-[var(--panel)] p-4 text-[var(--ink)]">
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--line)] pb-3">
      <div>
        <p className="mb-1 text-[10px] font-bold uppercase tracking-wider text-lxcup-primary">Administration · SSH</p>
        <h2 id="terminal-title" className="mb-1">Terminal</h2>
        <p className="mb-0 text-sm text-[var(--muted)]">Direkte SSH-Sitzung über das registrierte Zielprofil. Eingaben und Ausgaben werden nicht protokolliert.</p>
      </div>
      <div className="flex items-center gap-2">
        <span aria-live="polite" className="text-xs text-[var(--muted)]">{terminalStateLabel(state)}</span>
        {state === "disconnected" || state === "error" ? <button className="inline-flex min-h-9 items-center justify-center border border-lxcup-primary bg-lxcup-primary px-3 py-2 text-xs font-semibold text-white hover:bg-blue-700" type="button" onClick={connect}>Verbinden</button> : null}
        {state === "connecting" ? <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)]" type="button" onClick={close}>Abbrechen</button> : null}
        {state === "connected" ? <button className="inline-flex min-h-9 items-center justify-center border border-[var(--line)] bg-[var(--panel)] px-3 py-2 text-xs font-semibold text-[var(--ink)] hover:border-[var(--error)] hover:text-[var(--error)]" type="button" onClick={close}>Trennen</button> : null}
      </div>
    </div>
    <p className="m-0 text-xs text-[var(--muted)]">Tastatureingaben werden direkt an eine SSH-PTY-Shell gesendet. Sitzung endet nach 10 Minuten Inaktivität oder spätestens nach 60 Minuten.</p>
    {error ? <p className="m-0 border border-[var(--error)] bg-[var(--error-soft)] p-3 text-sm text-[var(--error)]" role="alert">{error}</p> : null}
    <div ref={terminalHost} aria-label="SSH-Terminal" className="h-80 min-h-56 overflow-hidden border border-[var(--line)] bg-[var(--paper)] p-2 sm:h-96" />
  </section>;
}

function terminalTheme() {
  const styles = getComputedStyle(document.documentElement);
  return {
    background: styles.getPropertyValue("--paper").trim(),
    foreground: styles.getPropertyValue("--ink").trim(),
    cursor: styles.getPropertyValue("--primary").trim(),
    selectionBackground: styles.getPropertyValue("--primary-soft").trim(),
  };
}

function fitTerminal(addon: FitAddon) {
  try {
    addon.fit();
  } catch {
    // The element can briefly be hidden while a parent route is transitioning.
  }
}

function parseTerminalMessage(data: string): { type?: string; message?: string } | null {
  try {
    const value: unknown = JSON.parse(data);
    if (!value || typeof value !== "object") return null;
    const message = value as { type?: unknown; message?: unknown };
    return {
      type: typeof message.type === "string" ? message.type : undefined,
      message: typeof message.message === "string" ? message.message : undefined,
    };
  } catch {
    return null;
  }
}

function terminalStateLabel(state: TerminalState) {
  if (state === "connecting") return "Verbindung wird aufgebaut";
  if (state === "connected") return "Verbunden";
  if (state === "error") return "Fehler";
  return "Getrennt";
}
