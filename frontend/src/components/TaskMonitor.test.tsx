import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import { useState } from "react";
import { TaskMonitor, type GlobalEvent } from "./TaskMonitor";

function StatefulTaskMonitor({ events, onClear }: Readonly<{ events: GlobalEvent[]; onClear: () => void }>) {
  const [collapsed, setCollapsed] = useState(false);
  return <MemoryRouter><TaskMonitor events={events} onClear={onClear} collapsed={collapsed} onToggle={() => setCollapsed((value) => !value)} /></MemoryRouter>;
}

describe("TaskMonitor", () => {
  it("renders recent activity with safe navigation targets", () => {
    const events: GlobalEvent[] = [
      { id: "event-1", receivedAt: "2026-09-20T12:00:00.000Z", event: { type: "Status", payload: { resource: "container_action", resource_id: "101", state: "running" } } },
      { id: "event-2", receivedAt: "2026-09-20T12:01:00.000Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "job-1", state: "completed" } }, jobDetails: { operationLabel: "Agent installieren", resourceName: "lxcup-test" } },
    ];

    render(<StatefulTaskMonitor events={events} onClear={() => undefined} />);

    expect(screen.getByRole("complementary", { name: "Aktivitäten" })).toBeInTheDocument();
    expect(screen.getByText("Agent installieren · lxcup-test")).toBeInTheDocument();
    expect(screen.getByText("Erfolgreich")).toHaveAttribute("aria-label", "Jobstatus: Erfolgreich (completed)");
    expect(screen.getAllByRole("link", { name: "Öffnen" })[0]).toHaveAttribute("href", "/containers/101");
    expect(screen.getAllByRole("link", { name: "Öffnen" })[1]).toHaveAttribute("href", "/workflows/job-1");
  });

  it("renders empty state and all non-navigation event kinds", () => {
    const onClear = vi.fn();
    const { rerender } = render(<MemoryRouter><TaskMonitor events={[]} onClear={onClear} collapsed={false} onToggle={() => undefined} /></MemoryRouter>);
    expect(screen.getByText("Noch keine laufenden Aufgaben.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Aktivität leeren" })).toBeDisabled();
    rerender(<MemoryRouter><TaskMonitor events={[
      { id: "task", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Task", payload: { task_id: "t", state: "running" } } },
      { id: "log", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Log", payload: { source: "worker", message: "log message" } } },
      { id: "error", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Error", payload: { code: "e", message: "error message" } } },
    ]} onClear={onClear} collapsed={false} onToggle={() => undefined} /></MemoryRouter>);
    expect(screen.getByText("Task · running")).toBeInTheDocument();
    expect(screen.getByText("log message")).toBeInTheDocument();
    expect(screen.getByText("error message")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Aktivität leeren" }));
    expect(onClear).toHaveBeenCalled();
  });

  it("uses operation and resource-specific activity icons with safe fallbacks", () => {
    const events: GlobalEvent[] = [
      { id: "deploy", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "deploy", state: "applying" } }, jobDetails: { operationLabel: "Agent aktualisieren", resourceName: "host-a" } },
      { id: "health", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "health", state: "succeeded" } }, jobDetails: { operationLabel: "Agent reparieren", resourceName: "host-a" } },
      { id: "inventory", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "inventory", state: "queued" } }, jobDetails: { operationLabel: "Paketinventar sammeln", resourceName: "host-a" } },
      { id: "packages", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "packages", state: "failed" } }, jobDetails: { operationLabel: "Pakete aktualisieren", resourceName: "host-a" } },
      { id: "configure", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "configure", state: "checking" } }, jobDetails: { operationLabel: "Ziel konfigurieren", resourceName: "host-a" } },
      { id: "workflow", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "workflow", state: "planned" } } },
      { id: "server", receivedAt: "2026-09-20T12:00:00Z", event: { type: "Status", payload: { resource: "target", resource_id: "host-a", state: "custom" } } },
    ];
    render(<MemoryRouter><TaskMonitor events={events} onClear={() => undefined} collapsed={false} onToggle={() => undefined} /></MemoryRouter>);

    for (const label of ["Agent aktualisieren", "Agent reparieren", "Paketinventar sammeln", "Pakete aktualisieren", "Ziel konfigurieren", "Workflow", "Status target"]) {
      expect(screen.getByTitle(label)).toHaveAttribute("aria-hidden", "true");
    }
    expect(screen.getByRole("status", { name: "Jobstatus: Geplant (planned)" })).toBeInTheDocument();
  });

  it("collapses to a narrow tab and can be reopened", () => {
    render(<StatefulTaskMonitor events={[]} onClear={() => undefined} />);

    fireEvent.click(screen.getByRole("button", { name: "Aktivitäten ausblenden" }));
    const reopenButton = screen.getByRole("button", { name: "Aktivitäten einblenden" });
    expect(reopenButton).toBeInTheDocument();
    expect(reopenButton).toHaveClass("w-7");
    const activitySidebar = document.querySelector('aside[aria-label="Aktivitäten"]');
    expect(activitySidebar).toHaveClass("fixed", "top-11", "right-0", "bottom-0", "transition-transform", "translate-x-full");
    expect(activitySidebar).toHaveAttribute("aria-hidden", "true");

    fireEvent.click(screen.getByRole("button", { name: "Aktivitäten einblenden" }));
    expect(screen.getByText("Noch keine laufenden Aufgaben.")).toBeInTheDocument();
  });
});
