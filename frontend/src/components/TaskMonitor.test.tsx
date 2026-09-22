import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
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
});
