import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "./SchedulesPage";

const mocks = vi.hoisted(() => ({
  setScheduleEnabled: vi.fn(async (id: string, enabled: boolean) => ({ id, enabled })),
  schedules: vi.fn(),
  targets: vi.fn(),
  policies: vi.fn(),
}));

vi.mock("../api", () => ({
  createSchedule: vi.fn(),
  setScheduleEnabled: mocks.setScheduleEnabled,
}));

vi.mock("../queries", () => ({
  queryKeys: { schedules: ["schedules"] },
  useSchedules: mocks.schedules,
  useTargets: mocks.targets,
  useUpdatePolicies: mocks.policies,
}));

describe("SchedulesPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.schedules.mockReturnValue({ isLoading: false, error: null, data: [{
      id: "docker-nightly",
      operation: "docker_discovery",
      timezone: "Europe/Berlin",
      target_ids: ["target-lxc"],
      every_minutes: 30,
      enabled: true,
      threshold: null,
      policy_id: null,
      last_run_at: null,
      next_run_at: "2026-01-01T01:00:00Z",
      last_error: null,
    }] });
    mocks.targets.mockReturnValue({ data: [{ id: "target-lxc", name: "test-lxc", kind: "lxc" }] });
    mocks.policies.mockReturnValue({ data: [] });
  });

  it("shows Docker discovery schedules and lets an administrator pause them", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><SchedulesPage /></MemoryRouter></QueryClientProvider>);
    expect(screen.getByText("Docker-Container erkennen")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pausieren" }));
    await waitFor(() => expect(mocks.setScheduleEnabled).toHaveBeenCalledWith("docker-nightly", false));
  });
});
