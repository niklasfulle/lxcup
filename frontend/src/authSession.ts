import { ApiError, apiClient, type AuthSession } from "./api";

export type InitialAuthCheck =
  | { kind: "authenticated"; session: AuthSession }
  | { kind: "unauthenticated"; accountAuth: boolean }
  | { kind: "unavailable" };

export async function checkInitialAuthSession(): Promise<InitialAuthCheck> {
  try {
    return { kind: "authenticated", session: await apiClient.getSession() };
  } catch (error: unknown) {
    if (!isUnauthorized(error)) return { kind: "unavailable" };
    try {
      const status = await apiClient.getAuthStatus();
      return { kind: "unauthenticated", accountAuth: status.enabled };
    } catch {
      return { kind: "unavailable" };
    }
  }
}

function isUnauthorized(error: unknown): error is ApiError {
  return error instanceof ApiError && error.status === 401;
}
