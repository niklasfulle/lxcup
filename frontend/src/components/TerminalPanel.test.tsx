import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalPanel } from "./TerminalPanel";

const terminalMocks = vi.hoisted(() => ({ instances: [] as Array<{
  cols: number;
  rows: number;
  output: string;
  input: (data: string) => void;
}> }));

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    output = "";
    private dataHandler: ((data: string) => void) | null = null;
    private resizeHandler: ((size: { cols: number; rows: number }) => void) | null = null;
    constructor() { terminalMocks.instances.push(this); }
    loadAddon(addon: { activate?: (terminal: { resize: (cols: number, rows: number) => void }) => void }) { addon.activate?.(this); }
    open() {}
    focus() {}
    dispose() {}
    write(data: string | Uint8Array) { this.output += typeof data === "string" ? data : new TextDecoder().decode(data); }
    onData(handler: (data: string) => void) {
      this.dataHandler = handler;
      return { dispose: () => { this.dataHandler = null; } };
    }
    onResize(handler: (size: { cols: number; rows: number }) => void) {
      this.resizeHandler = handler;
      return { dispose: () => { this.resizeHandler = null; } };
    }
    resize(cols: number, rows: number) {
      this.cols = cols;
      this.rows = rows;
      this.resizeHandler?.({ cols, rows });
    }
    input(data: string) { this.dataHandler?.(data); }
  },
}));

vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    private terminal: { resize: (cols: number, rows: number) => void } | null = null;
    activate(terminal: { resize: (cols: number, rows: number) => void }) { this.terminal = terminal; }
    fit() { this.terminal?.resize(100, 30); }
  },
}));

class TestWebSocket extends EventTarget {
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: TestWebSocket[] = [];
  binaryType = "blob";
  readyState = 0;
  sent: string[] = [];
  constructor(readonly url: string) {
    super();
    TestWebSocket.instances.push(this);
  }
  send(data: string) { this.sent.push(data); }
  close() { this.readyState = TestWebSocket.CLOSED; this.dispatchEvent(new Event("close")); }
  receive(data: string) { this.dispatchEvent(new MessageEvent("message", { data })); }
}

describe("TerminalPanel", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    terminalMocks.instances = [];
    TestWebSocket.instances = [];
  });

  it("waits for SSH readiness, forwards terminal input/resizes, and displays remote output", () => {
    vi.stubGlobal("WebSocket", TestWebSocket);
    render(<TerminalPanel targetId="target-1" />);

    fireEvent.click(screen.getByRole("button", { name: "Verbinden" }));
    const socket = TestWebSocket.instances[0];
    expect(socket.url).toBe(`${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/api/v1/targets/target-1/terminal`);
    expect(screen.getByText("Verbindung wird aufgebaut")).toBeInTheDocument();

    socket.readyState = TestWebSocket.OPEN;
    act(() => socket.receive('{"type":"ready"}'));
    const terminal = terminalMocks.instances[0];
    act(() => terminal.input("id\n"));
    expect(socket.sent).toEqual([
      JSON.stringify({ type: "resize", columns: 100, rows: 30 }),
      JSON.stringify({ type: "input", data: "id\n" }),
    ]);

    act(() => socket.dispatchEvent(new MessageEvent("message", { data: new TextEncoder().encode("uid=1000(test)\r\n").buffer })));
    expect(terminal.output).toContain("uid=1000(test)");
  });

  it("surfaces a connection error and reliably closes an active session", () => {
    vi.stubGlobal("WebSocket", TestWebSocket);
    render(<TerminalPanel targetId="target-2" />);
    fireEvent.click(screen.getByRole("button", { name: "Verbinden" }));
    const socket = TestWebSocket.instances[0];
    act(() => socket.receive('{"type":"error","message":"SSH-Verbindung fehlgeschlagen."}'));
    expect(screen.getByRole("alert")).toHaveTextContent("SSH-Verbindung fehlgeschlagen.");
    expect(socket.readyState).toBe(TestWebSocket.CLOSED);

    fireEvent.click(screen.getByRole("button", { name: "Verbinden" }));
    const nextSocket = TestWebSocket.instances.at(-1)!;
    nextSocket.readyState = TestWebSocket.OPEN;
    act(() => nextSocket.receive('{"type":"ready"}'));
    fireEvent.click(screen.getByRole("button", { name: "Trennen" }));
    expect(nextSocket.sent).toContain(JSON.stringify({ type: "close" }));
    expect(screen.getByText("Getrennt")).toBeInTheDocument();
  });
});
