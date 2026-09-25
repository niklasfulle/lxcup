import type { AnsibleJobDto, ApiEvent, ContainerDto, TargetDto } from "./api";
import type { GlobalEvent } from "./components/TaskMonitor";

const ACTIVITY_STORAGE_KEY = "lxcup-activity-v1";
const MAX_STORED_EVENTS = 40;

export function loadActivityEvents(): GlobalEvent[] {
  try {
    const raw = globalThis.localStorage?.getItem(ACTIVITY_STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isStoredActivity).slice(0, MAX_STORED_EVENTS);
  } catch {
    return [];
  }
}

export function saveActivityEvents(events: GlobalEvent[]) {
  try {
    // Persist state-only activity; raw worker logs and error messages may contain sensitive diagnostics.
    const safeEvents = events.filter(({ event }) => isPersistentActivity(event));
    globalThis.localStorage?.setItem(ACTIVITY_STORAGE_KEY, JSON.stringify(safeEvents.slice(0, MAX_STORED_EVENTS)));
  } catch {
    // Activity history is best-effort when browser storage is unavailable or full.
  }
}

export function syncJobActivity(events: GlobalEvent[], jobs: AnsibleJobDto[], targets: TargetDto[], containers: ContainerDto[]): GlobalEvent[] {
  const jobEvents = [...jobs]
    .sort((left, right) => right.updated_at.localeCompare(left.updated_at))
    .slice(0, MAX_STORED_EVENTS)
    .map((job): GlobalEvent => ({
      id: `job-status-${job.id}`,
      receivedAt: job.updated_at,
      event: { type: "Status", payload: { resource: "ansible_job", resource_id: job.id, state: job.status } },
      jobDetails: {
        operationLabel: jobOperationLabel(job.operation),
        resourceName: jobResourceName(job, targets, containers),
      },
    }));
  const otherEvents = events.filter(({ event }) => event.type !== "Status" || event.payload.resource !== "ansible_job");
  return [...jobEvents, ...otherEvents]
    .sort((left, right) => right.receivedAt.localeCompare(left.receivedAt))
    .slice(0, MAX_STORED_EVENTS);
}

function isStoredActivity(value: unknown): value is GlobalEvent {
  if (!isRecord(value) || typeof value.id !== "string" || typeof value.receivedAt !== "string" || !isRecord(value.event)) return false;
  if (value.jobDetails !== undefined && (!isRecord(value.jobDetails) || !isString(value.jobDetails.operationLabel) || !isString(value.jobDetails.resourceName))) return false;
  const event = value.event;
  if (event.type === "Task") return isRecord(event.payload) && isString(event.payload.task_id) && isString(event.payload.state);
  if (event.type === "Status") return isRecord(event.payload) && isString(event.payload.resource) && isString(event.payload.resource_id) && isString(event.payload.state);
  return false;
}

function jobOperationLabel(operation: string) {
  const labels: Record<string, string> = {
    deploy_agent: "Agent installieren",
    update_agent: "Agent aktualisieren",
    repair_agent: "Agent reparieren",
    update_packages: "Pakete aktualisieren",
    configure_target: "Ziel konfigurieren",
    health_check: "Healthcheck",
    collect_package_inventory: "Paketinventar sammeln",
  };
  return labels[operation] ?? operation.replaceAll("_", " ");
}

function jobResourceName(job: AnsibleJobDto, targets: TargetDto[], containers: ContainerDto[]) {
  const target = job.target;
  if ("target" in target) return targets.find(({ id }) => id === target.target)?.name ?? "Ziel";
  if ("container" in target) return containers.find(({ id }) => id === target.container)?.name ?? "LXC-Container";
  return target.node;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isPersistentActivity(event: ApiEvent) {
  return event.type === "Status" || event.type === "Task";
}
