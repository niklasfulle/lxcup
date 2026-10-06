import { describe, expect, it } from "vitest";
import { targetDisplayLabel } from "./targetLabel";

describe("targetDisplayLabel", () => {
  it("disambiguates equal target names by kind and address", () => {
    const linux = targetDisplayLabel({ name: "test", kind: "linux_server", address: "192.0.2.10" });
    const windows = targetDisplayLabel({ name: "test", kind: "windows_server", address: "192.0.2.20" });

    expect(linux).toBe("test · Linux-Server · 192.0.2.10");
    expect(windows).toBe("test · Windows-System · 192.0.2.20");
    expect(linux).not.toBe(windows);
  });
});
