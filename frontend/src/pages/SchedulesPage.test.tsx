import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SchedulesPage } from "./SchedulesPage";

const mocks = vi.hoisted(() => ({
  setScheduleEnabled: vi.fn(async (id: string, enabled: boolean) => ({ id, enabled })),
  createSchedule: vi.fn(async (request: unknown) => request),
  listSecrets: vi.fn(async () => [] as unknown[]),
  schedules: vi.fn(),
  targets: vi.fn(),
  policies: vi.fn(),
}));

vi.mock("../api", () => ({
  createSchedule: mocks.createSchedule,
  listSecrets: mocks.listSecrets,
  setScheduleEnabled: mocks.setScheduleEnabled,
}));

vi.mock("../queries", () => ({
  queryKeys: { schedules: ["schedules"], secrets: ["secrets"] },
  useSchedules: mocks.schedules,
  useTargets: mocks.targets,
  useUpdatePolicies: mocks.policies,
}));

describe("SchedulesPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.listSecrets.mockResolvedValue([]);
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
    mocks.targets.mockReturnValue({ data: [{ id: "target-lxc", name: "test-lxc", kind: "lxc", address: "192.0.2.10" }] });
    mocks.policies.mockReturnValue({ data: [] });
  });

  it("shows Docker discovery schedules and lets an administrator pause them", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><SchedulesPage /></MemoryRouter></QueryClientProvider>);
    expect(screen.getByText("Docker-Container erkennen")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pausieren" }));
    await waitFor(() => expect(mocks.setScheduleEnabled).toHaveBeenCalledWith("docker-nightly", false));
  });

  it("lets admins schedule encrypted backups with a global passphrase Secret", async () => {
    mocks.listSecrets.mockResolvedValue([{
      metadata: {
        metadata: { id: "backup-secret", name: "Offsite-Backup", kind: "backup_passphrase", scope: { type: "global" }, created_at: "2026-01-01", updated_at: "2026-01-01" },
        status: "active",
      },
    }]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><SchedulesPage isAdmin /></MemoryRouter></QueryClientProvider>);

    fireEvent.change(screen.getByPlaceholderText("nightly-inventory"), { target: { value: "nightly-backup" } });
    fireEvent.change(screen.getByRole("combobox", { name: "Aufgabe" }), { target: { value: "create_backup" } });
    await screen.findByRole("option", { name: "Offsite-Backup" });
    fireEvent.change(screen.getAllByRole("combobox")[1], { target: { value: "backup-secret" } });
    fireEvent.click(screen.getByRole("button", { name: /Zeitplan speichern/ }));

    await waitFor(() => expect(mocks.createSchedule).toHaveBeenCalledWith(expect.objectContaining({
      id: "nightly-backup",
      operation: "create_backup",
      timezone: "UTC",
      target_ids: [],
      every_minutes: 60,
      backup_secret_ref: "backup-secret",
    })));
  });

  it("does not offer backup schedules to non-admin users", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    render(<QueryClientProvider client={client}><MemoryRouter><SchedulesPage /></MemoryRouter></QueryClientProvider>);

    expect(screen.queryByRole("option", { name: "Backup erstellen" })).not.toBeInTheDocument();
    expect(mocks.listSecrets).not.toHaveBeenCalled();
  });
});
