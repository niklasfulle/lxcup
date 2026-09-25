import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadActivityEvents, saveActivityEvents, syncJobActivity } from "./activityPersistence";
import type { GlobalEvent } from "./components/TaskMonitor";

describe("activity persistence", () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.restoreAllMocks());

  it("restores task and status activity while excluding raw logs and errors", () => {
    const events: GlobalEvent[] = [
      { id: "failed-job", receivedAt: "2026-09-25T12:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "job-1", state: "failed" } } },
      { id: "task", receivedAt: "2026-09-25T12:01:00Z", event: { type: "Task", payload: { task_id: "task-1", state: "running" } } },
      { id: "log", receivedAt: "2026-09-25T12:02:00Z", event: { type: "Log", payload: { source: "worker", message: "sensitive worker output" } } },
      { id: "error", receivedAt: "2026-09-25T12:03:00Z", event: { type: "Error", payload: { code: "internal", message: "sensitive error detail" } } },
    ];

    saveActivityEvents(events);

    expect(loadActivityEvents()).toEqual(events.slice(0, 2));
    expect(localStorage.getItem("lxcup-activity-v1")).not.toContain("sensitive");
  });

  it("ignores malformed stored activity", () => {
    localStorage.setItem("lxcup-activity-v1", "{not json");
    expect(loadActivityEvents()).toEqual([]);
  });

  it("rejects non-array data and malformed task, status, and detail payloads", () => {
    localStorage.setItem("lxcup-activity-v1", JSON.stringify({ id: "not-an-array" }));
    expect(loadActivityEvents()).toEqual([]);

    const validBase = { id: "event", receivedAt: "2026-09-25T12:00:00Z" };
    localStorage.setItem("lxcup-activity-v1", JSON.stringify([
      null,
      { ...validBase, id: 42, event: { type: "Task", payload: { task_id: "task", state: "running" } } },
      { ...validBase, event: { type: "Task", payload: { task_id: 2, state: "running" } } },
      { ...validBase, event: { type: "Status", payload: { resource: "target", resource_id: "target-1", state: null } } },
      { ...validBase, event: { type: "Log", payload: { message: "not persisted" } } },
      { ...validBase, event: { type: "Status", payload: { resource: "target", resource_id: "target-1", state: "managed" } }, jobDetails: { operationLabel: "Healthcheck", resourceName: null } },
    ]));
    expect(loadActivityEvents()).toEqual([]);

    localStorage.setItem("lxcup-activity-v1", JSON.stringify([{ ...validBase, event: { type: "Status", payload: { resource: "target", resource_id: "target-1", state: "managed" } }, jobDetails: { operationLabel: "Healthcheck", resourceName: "test-target" } }]));
    expect(loadActivityEvents()).toHaveLength(1);
  });

  it("keeps browser-storage failures best-effort", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("storage blocked"); });
    expect(loadActivityEvents()).toEqual([]);

    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage full"); });
    expect(() => saveActivityEvents([])).not.toThrow();
  });

  it("replaces stale job statuses with current API states while keeping task activity", () => {
    const stale: GlobalEvent[] = [
      { id: "old-queued", receivedAt: "2026-09-25T11:00:00Z", event: { type: "Status", payload: { resource: "ansible_job", resource_id: "job-1", state: "queued" } } },
      { id: "task", receivedAt: "2026-09-25T11:30:00Z", event: { type: "Task", payload: { task_id: "task-1", state: "running" } } },
    ];
    const currentJob = { id: "job-1", operation: "deploy_agent", target: { target: "target-1" }, status: "succeeded", updated_at: "2026-09-25T12:00:00Z" } as any;

    const synced = syncJobActivity(stale, [currentJob], [{ id: "target-1", name: "lxcup-test" } as any], []);

    expect(synced).toHaveLength(2);
    expect(synced[0]).toMatchObject({ id: "job-status-job-1", event: { type: "Status", payload: { resource_id: "job-1", state: "succeeded" } } });
    expect(synced[0].jobDetails).toEqual({ operationLabel: "Agent installieren", resourceName: "lxcup-test" });
    expect(synced[1]).toEqual(stale[1]);
  });

  it("maps target, container, node, unknown operations, and preserves non-job statuses", () => {
    const updatedAt = "2026-09-25T12:00:00Z";
    const jobs = [
      { id: "missing-target", operation: "unknown_operation", target: { target: "missing" }, status: "failed", updated_at: updatedAt },
      { id: "container", operation: "health_check", target: { container: 101 }, status: "succeeded", updated_at: "2026-09-25T11:59:00Z" },
      { id: "missing-container", operation: "update_packages", target: { container: 404 }, status: "queued", updated_at: "2026-09-25T11:58:00Z" },
      { id: "node", operation: "health_check", target: { node: "node-1" }, status: "checking", updated_at: "2026-09-25T11:57:00Z" },
    ] as any;
    const unrelated: GlobalEvent[] = [
      { id: "target-status", receivedAt: updatedAt, event: { type: "Status", payload: { resource: "target", resource_id: "target-1", state: "managed" } } },
      { id: "task", receivedAt: updatedAt, event: { type: "Task", payload: { task_id: "task-1", state: "running" } } },
      { id: "old-job-status", receivedAt: updatedAt, event: { type: "Status", payload: { resource: "ansible_job", resource_id: "old-job", state: "queued" } } },
    ];

    const synced = syncJobActivity(unrelated, jobs, [], [{ id: 101, name: "web-lxc" } as any]);

    expect(synced.flatMap(({ jobDetails }) => jobDetails ? [jobDetails] : [])).toEqual([
      { operationLabel: "unknown operation", resourceName: "Ziel" },
      { operationLabel: "Healthcheck", resourceName: "web-lxc" },
      { operationLabel: "Pakete aktualisieren", resourceName: "LXC-Container" },
      { operationLabel: "Healthcheck", resourceName: "node-1" },
    ]);
    expect(synced.map(({ id }) => id)).toContain("target-status");
    expect(synced.map(({ id }) => id)).toContain("task");
    expect(synced.map(({ id }) => id)).not.toContain("old-job-status");
  });
});
