import servedScript from "../public/bootstrap-lxcup-user.sh?raw";
import { describe, expect, it } from "vitest";

describe("standalone host preparation script", () => {
  it("serves a standalone script with privilege detection and local SSH host keys", () => {
    const served = servedScript.replaceAll("\r\n", "\n");
    expect(served).not.toMatch(/scripts\//);
    expect(served).toContain('/etc/ssh/ssh_host_*_key.pub');
    expect(served).toContain('exec sudo -- bash "$0" "$@"');
  });
});
