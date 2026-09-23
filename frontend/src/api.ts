export type ApiEnvelope<T> = { data: T; request_id: string };

export type ContainerDto = {
  id: number;
  node_id: string;
  name: string;
  operating_system: string;
  status: string;
  management_state: string;
  discovered_at: string;
};
export type TargetKind = "lxc" | "linux_server" | "windows_server";
export type TargetTransport = "ssh" | "winrm";
export type TargetState = "pending" | "managed" | "disabled";
export type TargetDto = { id: string; name: string; kind: TargetKind; address: string; transport: TargetTransport; ssh_user?: string | null; credential_secret_ref: string; ssh_known_hosts_secret_ref?: string | null; agent_secret_ref: string; state: TargetState; created_at: string; updated_at: string };
export type CreateTargetRequest = { name: string; kind: TargetKind; address: string; transport: TargetTransport; ssh_user?: string | null; credential_secret_ref: string; ssh_known_hosts_secret_ref?: string | null; agent_secret_ref: string };
export type EnrollmentState = "requested" | "discovering" | "installing_agent" | "registering_agent" | "connected" | "failed" | "disabled";
export type EnrollmentDto = { id: string; container_id: number; state: EnrollmentState; failure_reason: string | null; created_at: string; updated_at: string };

export type ContainerAction = "start" | "stop" | "shutdown" | "reboot" | "refresh" | "clone" | "backup" | "restore" | "delete";
export type ContainerActionTaskDto = {
  id: string;
  container_id: number;
  action: ContainerAction;
  status: "queued" | "running" | "succeeded" | "failed" | "blocked";
  idempotency_key: string;
  error: string | null;
  created_at: string;
  updated_at: string;
};
export type AgentHealthDto = { healthy: boolean; info: { agent_id: string; platform: string; hostname: string; version: string; protocol_version: string }; metrics: { collected_at: string; commands_total: number; commands_failed: number; last_command_at: string | null } };

export type ScanDto = {
  id: string;
  container_id: number;
  status: string;
  update_count: number;
  created_at: string;
  completed_at: string | null;
};

export type PlanDto = {
  id: string;
  container_id: number;
  status: string;
  requested_packages: string[];
  change_count: number;
  plan_hash: string | null;
  created_at: string;
};

export type ExecutionDto = {
  id: string;
  plan_id: string;
  status: string;
  started_at: string | null;
  finished_at: string | null;
};

export type ExecutionSafetyDto = {
  snapshot: "NotRequested" | { Succeeded: { task_id: string } } | { Failed: { reason: string } };
  healthchecks: Array<{ check: { kind: string; [key: string]: unknown }; healthy: boolean; detail: string; checked_at: string }>;
  reboot: "Required" | "NotRequired" | "Unknown";
  automatic_rollback: boolean;
};

export type AnsibleOperation = "deploy_agent" | "update_agent" | "repair_agent" | "update_packages" | "configure_target" | "health_check";
export type AnsibleExecutionMode = "check" | "plan" | "apply" | "reconcile";
export type AnsibleJobDto = {
  id: string;
  operation: AnsibleOperation;
  playbook: string;
  playbook_version: string;
  target: { container: number } | { node: string } | { target: string };
  mode: AnsibleExecutionMode;
  status: string;
  parameter_hash: string;
  created_at: string;
  updated_at: string;
};

export type AnsibleJobEvent = {
  sequence: number;
  job_id: string;
  event: {
    kind: "queued" | "status_changed" | "task_started" | "task_finished" | "failed" | "reconcile_required" | "worker_log";
    status?: string;
    task?: string;
    changed?: boolean;
    code?: string;
    source?: string;
    message?: string;
  };
  created_at: string;
};

export type DockerWorkloadDto = {
  host_container_id: number;
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  management_state: "discovered" | "managed";
  discovered_at: string;
};

export type CreateAnsibleJobRequest = {
  operation: AnsibleOperation;
  target_id?: string;
  /** Legacy field for pre-target APIs; new callers use target_id. */
  container_id?: number;
  mode: AnsibleExecutionMode;
  parameters: Record<string, unknown>;
  idempotency_key: string;
  confirmed: boolean;
};

export type SecretKind = "ssh_private_key" | "ssh_password" | "ssh_known_hosts" | "agent_token" | "generic";
export type SecretScope = { type: "global" } | { type: "node"; id: string } | { type: "container"; id: number };
export type SecretMetadata = {
  metadata: {
    metadata: {
      id: string;
      name: string;
      kind: SecretKind;
      scope: SecretScope;
      created_at: string;
      updated_at: string;
    };
    status: "active" | "revoked";
  };
};
export type SecretAuditEvent = { secret_id: string; action: string; role: string; occurred_at: string };

export type ApiErrorBody = {
  error?: { code: string; message: string };
  request_id?: string;
};

export type ApiEvent =
  | { type: "Task"; payload: { task_id: string; state: string } }
  | { type: "Log"; payload: { source: string; message: string } }
  | { type: "Status"; payload: { resource: string; resource_id: string; state: string } }
  | { type: "Error"; payload: { code: string; message: string; request_id?: string } };

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly code: string,
    public readonly requestId?: string,
    public readonly retryable = false,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export class ApiClient {
  constructor(private readonly baseUrl = import.meta.env.VITE_API_BASE_URL ?? "") {}

  async get<T>(path: string, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, { signal });
  }

  async post<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, {
      method: "POST",
      signal,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  }

  async delete<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>(path, {
      method: "DELETE",
      signal,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  }

  subscribe(onEvent: (event: ApiEvent) => void, onError?: () => void): () => void {
    const source = new EventSource(`${this.baseUrl}/api/v1/events`);
    const eventTypes = ["task", "log", "status", "error"] as const;
    const handlers = eventTypes.map((type) => {
      const handler = (event: MessageEvent<string>) => {
        try {
          onEvent(JSON.parse(event.data) as ApiEvent);
        } catch {
          onError?.();
        }
      };
      source.addEventListener(type, handler);
      return [type, handler] as const;
    });
    source.onerror = () => onError?.();
    return () => {
      handlers.forEach(([type, handler]) => source.removeEventListener(type, handler));
      source.close();
    };
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    const maxAttempts = 3;
    for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
      try {
        return await this.requestAttempt<T>(path, init, attempt, maxAttempts);
      } catch (error) {
        if (error instanceof ApiError || attempt + 1 >= maxAttempts) throw error;
        await delay(attempt);
      }
    }
    throw new ApiError("Die API-Anfrage ist fehlgeschlagen.", 0, "api_error", undefined, true);
  }

  private async requestAttempt<T>(path: string, init: RequestInit | undefined, attempt: number, maxAttempts: number): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      cache: "no-store",
      headers: { accept: "application/json", ...init?.headers },
    });
    const text = await response.text();
    if (response.status === 204) return undefined as T;
    const payload = parsePayload<T>(text);
    if (!response.ok) {
      const error = payload as ApiErrorBody | undefined;
      const retryable = response.status >= 500;
      if (retryable && attempt + 1 < maxAttempts) {
        await delay(attempt);
        return this.requestAttempt<T>(path, init, attempt + 1, maxAttempts);
      }
      throw new ApiError(error?.error?.message ?? "Die API-Anfrage ist fehlgeschlagen.", response.status, error?.error?.code ?? "api_error", error?.request_id, retryable);
    }
    if (payload === undefined || !("data" in payload)) {
      throw new ApiError("Die API hat eine ungültige Antwort geliefert.", response.status, "invalid_response");
    }
    return payload.data;
  }
}

function parsePayload<T>(text: string): ApiEnvelope<T> | ApiErrorBody | undefined {
  if (!text) return undefined;
  try {
    return JSON.parse(text) as ApiEnvelope<T> | ApiErrorBody;
  } catch {
    return undefined;
  }
}

function delay(attempt: number): Promise<void> {
  return new Promise((resolve) => globalThis.setTimeout(resolve, 150 * 2 ** attempt));
}

export const apiClient = new ApiClient();

export function listTargets(signal?: AbortSignal) { return apiClient.get<TargetDto[]>("/api/v1/targets", signal); }
export function createTarget(request: CreateTargetRequest, signal?: AbortSignal) { return apiClient.post<TargetDto>("/api/v1/targets", request, signal); }

export function createAnsibleJob(request: CreateAnsibleJobRequest, signal?: AbortSignal) {
  return apiClient.post<AnsibleJobDto>("/api/v1/ansible/jobs", request, signal);
}

export function listAnsibleJobs(signal?: AbortSignal) { return apiClient.get<AnsibleJobDto[]>("/api/v1/ansible/jobs", signal); }
export function getAnsibleJob(id: string, signal?: AbortSignal) { return apiClient.get<AnsibleJobDto>(`/api/v1/ansible/jobs/${id}`, signal); }
export function getAnsibleJobEvents(id: string, signal?: AbortSignal) { return apiClient.get<AnsibleJobEvent[]>(`/api/v1/ansible/jobs/${id}/events`, signal); }

export function createEnrollment(containerId: number, targetId: string | undefined, startOnboarding = true, idempotencyKey = crypto.randomUUID(), signal?: AbortSignal) {
  return apiClient.post<EnrollmentDto>("/api/v1/enrollments", { container_id: containerId, target_id: targetId, idempotency_key: idempotencyKey, start_onboarding: startOnboarding }, signal);
}

export function getEnrollment(id: string, signal?: AbortSignal) {
  return apiClient.get<EnrollmentDto>(`/api/v1/enrollments/${id}`, signal);
}

export function listSecrets(signal?: AbortSignal) {
  return apiClient.get<SecretMetadata[]>("/api/v1/secrets", signal);
}

export function listSecretAudit(signal?: AbortSignal) {
  return apiClient.get<SecretAuditEvent[]>("/api/v1/secrets/audit", signal);
}

export function createSecret(request: { name: string; kind: SecretKind; scope: SecretScope; value: string }, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>("/api/v1/secrets", request, signal);
}

export function rotateSecret(id: string, value: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>(`/api/v1/secrets/${id}/rotate`, { value, confirmed }, signal);
}

export function revokeSecret(id: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.post<SecretMetadata>(`/api/v1/secrets/${id}/revoke`, { confirmed }, signal);
}

export function deleteSecret(id: string, confirmed = true, signal?: AbortSignal) {
  return apiClient.delete<void>(`/api/v1/secrets/${id}`, { confirmed }, signal);
}

export function createContainerAction(containerId: number, action: ContainerAction, confirmed = false, signal?: AbortSignal) {
  return apiClient.post<ContainerActionTaskDto>(`/api/v1/containers/${containerId}/actions`, { action, confirmed, idempotency_key: crypto.randomUUID() }, signal);
}

export function getAgentHealth(containerId: number, signal?: AbortSignal) {
  return apiClient.get<AgentHealthDto>(`/api/v1/containers/${containerId}/agent/health`, signal);
}

export function getAgentMetrics(containerId: number, signal?: AbortSignal) {
  return apiClient.get<AgentHealthDto["metrics"]>(`/api/v1/containers/${containerId}/agent/metrics`, signal);
}

export function listDockerWorkloads(containerId: number, signal?: AbortSignal) {
  return apiClient.get<DockerWorkloadDto[]>(`/api/v1/containers/${containerId}/docker/containers`, signal);
}

export function discoverDockerWorkloads(containerId: number, signal?: AbortSignal) {
  return apiClient.post<DockerWorkloadDto[]>(`/api/v1/containers/${containerId}/docker/discover`, undefined, signal);
}

export function adoptDockerWorkload(containerId: number, dockerId: string, signal?: AbortSignal) {
  return apiClient.post<DockerWorkloadDto>(`/api/v1/containers/${containerId}/docker/containers/${encodeURIComponent(dockerId)}/adopt`, undefined, signal);
}

export function removeDockerWorkload(containerId: number, dockerId: string, signal?: AbortSignal) {
  return apiClient.delete<void>(`/api/v1/containers/${containerId}/docker/containers/${encodeURIComponent(dockerId)}`, { confirmed: true }, signal);
}
