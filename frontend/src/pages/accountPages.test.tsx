import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import AccountPasswordPage from "./AccountPasswordPage";
import AuditLogPage from "./AuditLogPage";
import UsersPage from "./UsersPage";

const api = vi.hoisted(() => ({
  changePassword: vi.fn(async () => undefined),
  createUser: vi.fn(async (request: { username: string; role: string }) => ({
    id: "user-new", username: request.username, role: request.role, must_change_password: true, disabled: false,
    created_at: "2026-09-29T10:00:00Z", updated_at: "2026-09-29T10:00:00Z",
  })),
  deleteUser: vi.fn(async () => undefined),
  listUserAudit: vi.fn(async (filters: { limit: number; offset: number }) => ({ events: [{
    id: "event-1", actor_user_id: null as string | null, actor_username: "admin", actor_role: "admin" as const, action: "user.created", resource_type: "user",
    resource_id: "user-new", request_id: null as string | null, status_code: 201 as number, details: {}, created_at: "2026-09-29T10:00:00Z",
  }], total: 101, limit: filters.limit, offset: filters.offset })),
  listUsers: vi.fn(async () => [{
    id: "user-1", username: "operator", role: "user", must_change_password: false, disabled: false,
    created_at: "2026-09-29T10:00:00Z", updated_at: "2026-09-29T10:00:00Z",
  }]),
  resetUserPassword: vi.fn(async () => undefined),
  updateUser: vi.fn(async () => undefined),
}));

vi.mock("../api", () => ({
  ApiError: class ApiError extends Error {},
  changePassword: api.changePassword,
  createUser: api.createUser,
  deleteUser: api.deleteUser,
  listUserAudit: api.listUserAudit,
  listUsers: api.listUsers,
  resetUserPassword: api.resetUserPassword,
  updateUser: api.updateUser,
}));

function renderPage(page: React.ReactNode) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(<QueryClientProvider client={queryClient}>{page}</QueryClientProvider>);
}

afterEach(() => { cleanup(); vi.clearAllMocks(); });

describe("account and admin pages", () => {
  it("creates a local user and marks the first login password change as required", async () => {
    renderPage(<UsersPage />);
    await screen.findByText("operator");
    fireEvent.change(screen.getByLabelText("Benutzername"), { target: { value: "new-user" } });
    fireEvent.change(screen.getByLabelText(/Startpasswort/), { target: { value: "a sufficiently long password" } });
    fireEvent.click(screen.getByRole("button", { name: "Benutzer erstellen" }));
    await waitFor(() => expect(api.createUser).toHaveBeenCalledWith({ username: "new-user", password: "a sufficiently long password", role: "user" }));
  });

  it("keeps user-creation submit available and explains an invalid start password", async () => {
    renderPage(<UsersPage />);
    const submit = await screen.findByRole("button", { name: "Benutzer erstellen" });
    expect(submit).toBeEnabled();
    fireEvent.click(submit);
    expect(await screen.findByRole("alert")).toHaveTextContent("Benutzername darf nur");
    fireEvent.change(screen.getByLabelText("Benutzername"), { target: { value: "operator2" } });
    fireEvent.click(submit);
    expect(await screen.findByRole("alert")).toHaveTextContent("Startpasswort muss mindestens 12 Zeichen enthalten");
  });

  it("changes the signed-in user's password and confirms success", async () => {
    renderPage(<AccountPasswordPage />);
    const save = screen.getByRole("button", { name: "Passwort speichern" });
    expect(save).toBeEnabled();
    fireEvent.click(save);
    expect(await screen.findByRole("alert")).toHaveTextContent("aktuelles Passwort eingeben");
    const currentPassword = screen.getByLabelText("Aktuelles Passwort");
    fireEvent.change(currentPassword, { target: { value: "old-password" } });
    fireEvent.click(screen.getAllByRole("button", { name: "Passwort anzeigen" })[0]!);
    expect(currentPassword).toHaveAttribute("type", "text");
    expect(currentPassword).toHaveValue("old-password");
    fireEvent.click(screen.getByRole("button", { name: "Passwort verbergen" }));
    expect(currentPassword).toHaveAttribute("type", "password");
    fireEvent.change(screen.getAllByLabelText(/Neues Passwort/)[0]!, { target: { value: "new-password-123" } });
    fireEvent.change(screen.getByLabelText("Neues Passwort wiederholen"), { target: { value: "new-password-123" } });
    fireEvent.click(save);
    await waitFor(() => expect(api.changePassword).toHaveBeenCalledWith("new-password-123", "old-password"));
    expect(await screen.findByText("Dein Passwort wurde geändert.")).toBeInTheDocument();
  });

  it("renders actor and action details in the admin audit view", async () => {
    renderPage(<AuditLogPage />);
    expect(await screen.findByText("user.created")).toBeInTheDocument();
    expect(screen.getByText("admin")).toBeInTheDocument();
    expect(screen.getByText("user-new")).toBeInTheDocument();
    expect(screen.getByText("1–1 von 101")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Benutzer"), { target: { value: "admin" } });
    fireEvent.change(screen.getByLabelText("Aktion"), { target: { value: "user.created" } });
    fireEvent.click(screen.getByRole("button", { name: "Filter anwenden" }));
    await waitFor(() => expect(api.listUserAudit).toHaveBeenLastCalledWith(expect.objectContaining({ actor_username: "admin", action: "user.created", offset: 0 }), expect.any(AbortSignal)));
    await waitFor(() => expect(screen.getByRole("button", { name: "Weiter" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Weiter" }));
    await waitFor(() => expect(api.listUserAudit).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 50 }), expect.any(AbortSignal)));
    expect(await screen.findByText("51–51 von 101")).toBeInTheDocument();
  });

  it("marks an interrupted audit reservation as pending instead of successful", async () => {
    api.listUserAudit.mockResolvedValueOnce({ events: [{
      id: "event-pending", actor_user_id: null, actor_username: "admin", actor_role: "admin", action: "target.deleted", resource_type: "targets",
      resource_id: "target-1", request_id: "request-1", status_code: 102, details: { state: "pending" }, created_at: "2026-09-29T10:00:00Z",
    }], total: 1, limit: 50, offset: 0 });
    renderPage(<AuditLogPage />);
    const pending = await screen.findByText("Ausstehend");
    expect(pending).toHaveClass("text-[var(--warning)]");
    expect(pending).toHaveAttribute("title", expect.stringContaining("Ergebnis ist noch nicht gespeichert"));
  });
});
