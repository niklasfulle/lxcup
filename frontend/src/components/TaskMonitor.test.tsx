import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import { TaskMonitor, type GlobalEvent } from "./TaskMonitor";

describe("TaskMonitor", () => {
  it("renders recent activity with safe navigation targets", () => {
    const events: GlobalEvent[] = [
      { id: "event-1", receivedAt: "2026-09-20T12:00:00.000Z", event: { type: "Status", payload: { resource: "container_action", resource_id: "101", state: "running" } } },
      { id: "event-2", receivedAt: "2026-09-20T12:01:00.000Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "job-1", state: "completed" } } },
    ];

    render(<MemoryRouter><TaskMonitor events={events} onClear={() => undefined} /></MemoryRouter>);

    expect(screen.getByRole("region", { name: "Globale Aufgaben und Ereignisse" })).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: "Öffnen" })[0]).toHaveAttribute("href", "/containers/101");
    expect(screen.getAllByRole("link", { name: "Öffnen" })[1]).toHaveAttribute("href", "/workflows/job-1");
  });

  it("renders empty state and all non-navigation event kinds", () => {
    const onClear = vi.fn();
    const { rerender } = render(<MemoryRouter><TaskMonitor events={[]} onClear={onClear} /></MemoryRouter>);
    expect(screen.getByText("Noch keine laufenden Aufgaben.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Aktivität leeren" })).toBeDisabled();
    rerender(<MemoryRouter><TaskMonitor events={[
      { id: "task", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Task", payload: { task_id: "t", state: "running" } } },
      { id: "log", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Log", payload: { source: "worker", message: "log message" } } },
      { id: "error", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Error", payload: { code: "e", message: "error message" } } },
    ]} onClear={onClear} /></MemoryRouter>);
    expect(screen.getByText("Task · running")).toBeInTheDocument();
    expect(screen.getByText("log message")).toBeInTheDocument();
    expect(screen.getByText("error message")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Aktivität leeren" }));
    expect(onClear).toHaveBeenCalled();
  });
});
