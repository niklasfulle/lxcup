export type ApiEnvelope<T> = { data: T; request_id: string };

export type NodeDto = {
  id: string;
  name: string;
  address: string;
  status: string;
  proxmox_version: string | null;
  capabilities: string[];
  last_checked_at: string | null;
};

export type ContainerDto = {
  id: number;
  node_id: string;
  name: string;
  operating_system: string;
  status: string;
  management_state: string;
  discovered_at: string;
};

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

export type ApiErrorBody = {
  error?: { code: string; message: string };
  request_id?: string;
};

export type ApiEvent =
  | { type: "Task"; payload: { task_id: string; state: string } }
  | { type: "Log"; payload: { source: string; message: string } }
  | { type: "Status"; payload: { resource: string; resource_id: string; state: string } };

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly code: string,
    public readonly requestId?: string,
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

  subscribe(onEvent: (event: ApiEvent) => void, onError?: () => void): () => void {
    const source = new EventSource(`${this.baseUrl}/api/v1/events`);
    const eventTypes = ["task", "log", "status"] as const;
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
    const response = await fetch(`${this.baseUrl}${path}`, {
      ...init,
      headers: { accept: "application/json", ...init?.headers },
    });
    const payload = (await response.json()) as ApiEnvelope<T> | ApiErrorBody;
    if (!response.ok) {
      const error = payload as ApiErrorBody;
      throw new ApiError(
        error.error?.message ?? "Die API-Anfrage ist fehlgeschlagen.",
        response.status,
        error.error?.code ?? "api_error",
        error.request_id,
      );
    }
    return (payload as ApiEnvelope<T>).data;
  }
}

export const apiClient = new ApiClient();
