import { act, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import {
  useAnsibleJob,
  useAnsibleJobEvents,
  useAnsibleJobs,
  useDockerDiscovery,
  useDockerWorkloads,
  useEnrollment,
  useExecutionSafety,
  useTargets,
  usePackageInventory,
  useSchedules,
  useTargetDockerInventories,
  useTargetDockerInventory,
  useTargetDockerTelemetry,
  useTargetTelemetry,
  useTelemetryAlerts,
  useUpdatePolicies,
  useUserAudit,
  useUsers,
  useWorkerAvailability,
} from "./queries";
import * as api from "./api";

function Probe() {
  const targets = useTargets();
  const safety = useExecutionSafety("execution-1");
  const jobs = useAnsibleJobs();
  const job = useAnsibleJob("job-1");
  const events = useAnsibleJobEvents("job-1", false);
  const enrollment = useEnrollment("enrollment-1");
  const workloads = useDockerWorkloads(101);
  const schedules = useSchedules();
  const policies = useUpdatePolicies();
  return <output>{[targets, safety, jobs, job, events, enrollment, workloads, schedules, policies].filter((query) => query.isSuccess).length}</output>;
}

function DisabledProbe() {
  const safety = useExecutionSafety(undefined);
  const enrollment = useEnrollment(undefined);
  const job = useAnsibleJob(undefined);
  const events = useAnsibleJobEvents(undefined, true);
  const workloads = useDockerWorkloads(undefined);
  return <output>{[safety, enrollment, job, events, workloads].filter((query) => query.fetchStatus === "idle").length}</output>;
}

function TargetHeartbeatProbe() {
  const targets = useTargets();
  return <output>{targets.data?.[0]?.updated_at ?? "loading"}</output>;
}

function TargetDockerInventoriesProbe() {
  const inventories = useTargetDockerInventories(["target-1", "target-2"]);
  return <output>{inventories.map((inventory) => inventory.data?.target_id).filter(Boolean).join(",")}</output>;
}

function RemainingQueriesProbe() {
  const users = useUsers();
  const audit = useUserAudit();
  const worker = useWorkerAvailability();
  const discovery = useDockerDiscovery(101);
  const targetInventory = useTargetDockerInventory("target-1");
  const dockerTelemetry = useTargetDockerTelemetry("target-1");
  const packageInventory = usePackageInventory("target-1");
  const telemetry = useTargetTelemetry("target-1");
  const alerts = useTelemetryAlerts();
  return <output>{[users, audit, worker, discovery, targetInventory, dockerTelemetry, packageInventory, telemetry, alerts].filter((query) => query.isSuccess).length}</output>;
}

describe("query hooks", () => {
  it("load each resource through its API query function", async () => {
    vi.spyOn(api, "listTargets").mockResolvedValue([]);
    vi.spyOn(api.apiClient, "get").mockResolvedValue([] as never);
    vi.spyOn(api, "getAnsibleJob").mockResolvedValue({ id: "job-1" } as never);
    vi.spyOn(api, "getAnsibleJobEvents").mockResolvedValue([]);
    vi.spyOn(api, "getEnrollment").mockResolvedValue({ id: "enrollment-1", state: "connected" } as never);
    vi.spyOn(api, "listDockerWorkloads").mockResolvedValue([]);
    vi.spyOn(api, "listSchedules").mockResolvedValue([]);
    vi.spyOn(api, "listUpdatePolicies").mockResolvedValue([]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><Probe /></QueryClientProvider>);
    await waitFor(() => expect(screen.getByText("9")).toBeInTheDocument());
  });

  it("keeps disabled queries idle and polls active target/job states", async () => {
    vi.spyOn(api, "listTargets").mockResolvedValue([{ id: "t", state: "pending" } as never]);
    vi.spyOn(api, "listAnsibleJobs").mockResolvedValue([{ id: "j", status: "applying" } as never]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><><DisabledProbe /><Probe /></></QueryClientProvider>);
    await waitFor(() => expect(screen.getByText("5")).toBeInTheDocument());
  });

  it("refreshes managed targets so the latest heartbeat timestamp becomes visible", async () => {
    vi.useFakeTimers();
    let request = 0;
    const listTargets = vi.spyOn(api, "listTargets").mockImplementation(async () => {
      request += 1;
      return [{ id: "t", state: "managed", updated_at: request === 1 ? "heartbeat-1" : "heartbeat-2" }] as never;
    });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
    try {
      render(<QueryClientProvider client={client}><TargetHeartbeatProbe /></QueryClientProvider>);
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(screen.getByText("heartbeat-1")).toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
      expect(listTargets).toHaveBeenCalledTimes(2);
      await act(async () => { await vi.advanceTimersByTimeAsync(100); });
      expect(client.getQueryData(["targets"])).toEqual([{ id: "t", state: "managed", updated_at: "heartbeat-2" }]);
      expect(screen.getByText("heartbeat-2")).toBeInTheDocument();
    } finally {
      client.clear();
      vi.useRealTimers();
    }
  });

  it("loads the persisted Docker inventory for each registered target", async () => {
    const getInventory = vi.spyOn(api, "getTargetDockerInventory").mockImplementation(async (targetId) => ({
      target_id: targetId,
      target_name: `host-${targetId}`,
      available: true,
      reason: null,
      collected_at: "2026-01-01T00:00:00Z",
      containers: [],
    }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><TargetDockerInventoriesProbe /></QueryClientProvider>);

    expect(await screen.findByText("target-1,target-2")).toBeInTheDocument();
    expect(getInventory).toHaveBeenCalledWith("target-1", expect.any(AbortSignal));
    expect(getInventory).toHaveBeenCalledWith("target-2", expect.any(AbortSignal));
    client.clear();
  });

  it("loads account, worker, Docker and telemetry queries through their APIs", async () => {
    vi.spyOn(api, "listUsers").mockResolvedValue([]);
    vi.spyOn(api, "listUserAudit").mockResolvedValue({ entries: [], total: 0 } as never);
    vi.spyOn(api, "getWorkerAvailability").mockResolvedValue({ available: true } as never);
    vi.spyOn(api, "getDockerDiscovery").mockResolvedValue(null);
    vi.spyOn(api, "getTargetDockerInventory").mockResolvedValue({ target_id: "target-1", containers: [] } as never);
    vi.spyOn(api, "getTargetDockerTelemetry").mockResolvedValue({ target_id: "target-1", samples: [] } as never);
    vi.spyOn(api, "getPackageInventory").mockResolvedValue({ target_id: "target-1", packages: [] } as never);
    vi.spyOn(api, "getTargetTelemetry").mockResolvedValue({ target_id: "target-1", samples: [] } as never);
    vi.spyOn(api, "getTelemetryAlerts").mockResolvedValue([]);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><RemainingQueriesProbe /></QueryClientProvider>);

    expect(await screen.findByText("9")).toBeInTheDocument();
    client.clear();
  });
});
