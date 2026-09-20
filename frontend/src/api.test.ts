import { describe, expect, it, vi } from "vitest";
import { ApiClient, ApiError } from "./api";
import { buildWorkflowRequest } from "./pages/WorkflowsPage";

describe("ApiClient", () => {
  it("unwraps a versioned API envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: [{ name: "pve01" }], request_id: "req-1" }), { status: 200 })));
    await expect(new ApiClient().get<{ name: string }[]>("/api/v1/nodes")).resolves.toEqual([{ name: "pve01" }]);
  });

  it("exposes structured API errors without losing the request id", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: { code: "not_found", message: "container not found" }, request_id: "req-2" }), { status: 404 })));
    await expect(new ApiClient().get("/api/v1/containers")).rejects.toEqual(expect.objectContaining({ status: 404, code: "not_found", requestId: "req-2" }));
  });

  it("retries transient server failures before returning the envelope", async () => {
    vi.stubGlobal("fetch", vi.fn()
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: { code: "internal_error", message: "temporary" }, request_id: "req-3" }), { status: 503 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ data: { ok: true }, request_id: "req-4" }), { status: 200 })));

    await expect(new ApiClient().get<{ ok: boolean }>("/api/v1/health")).resolves.toEqual({ ok: true });
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("rejects successful responses without the versioned data envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("not-json", { status: 200 })));

    await expect(new ApiClient().get("/api/v1/nodes")).rejects.toEqual(expect.objectContaining({ code: "invalid_response", status: 200 }));
  });

  it("builds only allowlisted workflow fields without playbook or shell input", () => {
    const request = buildWorkflowRequest(101, "update_packages", "plan", "nginx, curl", false);

    expect(request).toMatchObject({
      operation: "update_packages",
      container_id: 101,
      mode: "plan",
      parameters: { operation: "update_packages", packages: ["nginx", "curl"] },
    });
    expect(request).not.toHaveProperty("playbook");
    expect(request).not.toHaveProperty("shell");
  });
});
