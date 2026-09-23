import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { MemoryRouter } from "react-router-dom";
import { TargetLifecycle } from "./TargetLifecycle";
import type { TargetDto } from "../api";

const pendingTarget: TargetDto = {
  id: "target-1",
  name: "web-lxc",
  kind: "lxc",
  address: "10.0.0.15",
  transport: "ssh",
  credential_secret_ref: "secret-1",
  agent_secret_ref: "secret-2",
  state: "pending",
  created_at: "2026-09-22T10:00:00.000Z",
  updated_at: "2026-09-22T10:00:00.000Z",
};

describe("TargetLifecycle", () => {
  it("explains pending targets and links to the next workflow", () => {
    render(<MemoryRouter><TargetLifecycle target={pendingTarget} /></MemoryRouter>);

    expect(screen.getByText("Wartet auf Agent")).toBeInTheDocument();
    expect(screen.getByText(/ersten Heartbeat sendet/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Agent-Workflow öffnen" })).toHaveAttribute("href", "/workflows?target=target-1");
  });

  it("renders managed and disabled status branches", () => {
    render(<MemoryRouter><TargetLifecycle target={{ ...pendingTarget, state: "managed", name: "managed" }} /><TargetLifecycle target={{ ...pendingTarget, state: "disabled", name: "disabled", kind: "linux_server" }} /></MemoryRouter>);
    expect(screen.getByText("Der LXC ist bereit.")).toBeInTheDocument();
    expect(screen.getByText("Das Ziel ist deaktiviert.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Workflows öffnen →" })).toHaveAttribute("href", "/workflows");
  });
});
