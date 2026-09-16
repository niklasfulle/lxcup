import { describe, expect, it, vi } from "vitest";
import { ApiClient, ApiError } from "./api";

describe("ApiClient", () => {
  it("unwraps a versioned API envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ data: [{ name: "pve01" }], request_id: "req-1" }), { status: 200 })));
    await expect(new ApiClient().get<{ name: string }[]>("/api/v1/nodes")).resolves.toEqual([{ name: "pve01" }]);
  });

  it("exposes structured API errors without losing the request id", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: { code: "not_found", message: "container not found" }, request_id: "req-2" }), { status: 404 })));
    await expect(new ApiClient().get("/api/v1/containers")).rejects.toEqual(expect.objectContaining({ status: 404, code: "not_found", requestId: "req-2" }));
  });
});
