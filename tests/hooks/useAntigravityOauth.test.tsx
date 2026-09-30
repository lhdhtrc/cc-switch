import type { ReactNode } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  antigravityAuthKeys,
  useAntigravityOauth,
} from "@/components/providers/forms/hooks/useAntigravityOauth";
import { useAntigravityOauthQuotaByAccountId } from "@/lib/query/subscription";
import type { AntigravityLogin } from "@/lib/api/antigravityAuth";

const api = vi.hoisted(() => ({
  status: vi.fn(),
  start: vi.fn(),
  poll: vi.fn(),
  cancel: vi.fn(),
  remove: vi.fn(),
  setDefault: vi.fn(),
  quota: vi.fn(),
  openExternal: vi.fn(),
}));
vi.mock("@/lib/api/antigravityAuth", () => ({ antigravityAuthApi: api }));
vi.mock("@/lib/api", () => ({
  settingsApi: { openExternal: api.openExternal },
}));

const login: AntigravityLogin = {
  flowId: "flow-1",
  authorizationUrl: "https://accounts.google.com/test",
  expiresAt: 1,
};
const account = {
  id: "acct-1",
  email: "user@example.test",
  authenticatedAt: 1,
  isDefault: true,
  requiresReauth: false,
};
const quota = {
  tool: "antigravity",
  credentialStatus: "valid",
  credentialMessage: null,
  success: true,
  tiers: [{ name: "gemini_pro", utilization: 40, resetsAt: null }],
  extraUsage: null,
  error: null,
  queriedAt: Date.now(),
};
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { client, wrapper };
}

describe("useAntigravityOauth", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    api.status.mockResolvedValue({
      accounts: [account],
      defaultAccountId: account.id,
    });
    api.start.mockResolvedValue(login);
    api.poll.mockResolvedValue({ status: "pending" });
    api.cancel.mockResolvedValue(undefined);
    api.remove.mockResolvedValue(undefined);
    api.setDefault.mockResolvedValue(undefined);
    api.openExternal.mockResolvedValue(undefined);
    api.quota.mockResolvedValue(quota);
  });

  it("opens login, reopens the browser, and cancels the specific flow", async () => {
    const { wrapper } = setup();
    const { result } = renderHook(() => useAntigravityOauth(), { wrapper });
    await act(async () => {
      await result.current.start();
    });
    expect(api.start).toHaveBeenCalledWith(null);
    expect(api.openExternal).toHaveBeenCalledWith(login.authorizationUrl);
    expect(result.current.busy).toBe(true);
    await act(async () => {
      await result.current.openBrowser();
    });
    expect(api.openExternal).toHaveBeenCalledTimes(2);
    await act(async () => {
      await result.current.cancel();
    });
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
    expect(result.current.login).toBeNull();
    expect(result.current.busy).toBe(false);
  });

  it("retains the pending flow when browser opening fails", async () => {
    api.openExternal.mockRejectedValueOnce(new Error("browser failed"));
    const { wrapper } = setup();
    const { result } = renderHook(() => useAntigravityOauth(), { wrapper });
    await act(async () => {
      await result.current.start("acct-1");
    });
    expect(api.start).toHaveBeenCalledWith("acct-1");
    expect(result.current.login?.flowId).toBe("flow-1");
    expect(result.current.error).toContain("browser failed");
    await act(async () => {
      await result.current.openBrowser();
    });
    expect(result.current.error).toBeNull();
  });

  it("cancels an abandoned start result after unmount", async () => {
    let resolve!: (value: AntigravityLogin) => void;
    api.start.mockImplementation(
      () =>
        new Promise<AntigravityLogin>((done) => {
          resolve = done;
        }),
    );
    const { wrapper } = setup();
    const { result, unmount } = renderHook(() => useAntigravityOauth(), {
      wrapper,
    });
    let started!: Promise<void>;
    act(() => {
      started = result.current.start();
    });
    unmount();
    await act(async () => {
      resolve(login);
      await started;
    });
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
    expect(api.openExternal).not.toHaveBeenCalled();
  });

  it("resyncs accounts and quota if authorization completes just before cancel", async () => {
    api.status.mockResolvedValue({ accounts: [], defaultAccountId: null });
    const { client, wrapper } = setup();
    client.setQueryData(antigravityAuthKeys.quota("acct-1"), quota);
    const { result } = renderHook(() => useAntigravityOauth(), { wrapper });
    await waitFor(() => expect(result.current.status.isSuccess).toBe(true));
    await act(async () => {
      await result.current.start();
    });
    api.cancel.mockImplementation(async () => {
      api.status.mockResolvedValue({
        accounts: [account],
        defaultAccountId: account.id,
      });
    });
    await act(async () => {
      await result.current.cancel();
    });
    await waitFor(() =>
      expect(result.current.status.data?.accounts).toEqual([account]),
    );
    expect(
      client.getQueryData(antigravityAuthKeys.quota("acct-1")),
    ).toBeUndefined();
  });

  it("cancels the active flow when the auth panel unmounts", async () => {
    const { wrapper } = setup();
    const { result, unmount } = renderHook(() => useAntigravityOauth(), {
      wrapper,
    });
    await act(async () => {
      await result.current.start();
    });
    unmount();
    expect(api.cancel).toHaveBeenCalledWith("flow-1");
  });

  it("surfaces denied authorization and clears the flow", async () => {
    api.poll.mockResolvedValue({ status: "error", message: "denied" });
    const { wrapper } = setup();
    const { result } = renderHook(() => useAntigravityOauth(), { wrapper });
    await act(async () => {
      await result.current.start();
    });
    await waitFor(() => expect(result.current.error).toBe("denied"));
    expect(result.current.login).toBeNull();
  });

  it("successful reauthentication refetches the active account quota only", async () => {
    api.poll.mockResolvedValue({ status: "completed", account });
    const { client, wrapper } = setup();
    client.setQueryData(antigravityAuthKeys.quota("acct-2"), {
      ...quota,
      tool: "other-account",
    });
    const { result } = renderHook(
      () => ({
        auth: useAntigravityOauth(),
        quota: useAntigravityOauthQuotaByAccountId("acct-1"),
      }),
      { wrapper },
    );
    await waitFor(() => expect(result.current.quota.isSuccess).toBe(true));
    await act(async () => {
      await result.current.auth.start("acct-1");
    });
    await waitFor(() => expect(api.quota).toHaveBeenCalledTimes(2));
    expect(client.getQueryData(antigravityAuthKeys.quota("acct-2"))).toEqual({
      ...quota,
      tool: "other-account",
    });
    expect(result.current.auth.login).toBeNull();
  });

  it("isolates account quota caches and removes only the deleted account", async () => {
    const { client, wrapper } = setup();
    client.setQueryData(antigravityAuthKeys.quota("acct-1"), quota);
    client.setQueryData(antigravityAuthKeys.quota("acct-2"), quota);
    const { result } = renderHook(() => useAntigravityOauth(), { wrapper });
    act(() => result.current.setDefault("acct-2"));
    await waitFor(() => expect(api.setDefault).toHaveBeenCalledWith("acct-2"));
    await waitFor(() => expect(result.current.busy).toBe(false));
    act(() => result.current.remove("acct-1"));
    await waitFor(() => expect(api.remove).toHaveBeenCalledWith("acct-1"));
    await waitFor(() =>
      expect(
        client.getQueryData(antigravityAuthKeys.quota("acct-1")),
      ).toBeUndefined(),
    );
    expect(client.getQueryData(antigravityAuthKeys.quota("acct-2"))).toEqual(
      quota,
    );
  });
});
