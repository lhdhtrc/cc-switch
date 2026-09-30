import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AntigravityOAuthSection } from "@/components/providers/forms/AntigravityOAuthSection";

const mocks = vi.hoisted(() => ({ auth: vi.fn(), quota: vi.fn() }));
vi.mock("@/components/providers/forms/hooks/useAntigravityOauth", () => ({
  useAntigravityOauth: mocks.auth,
}));
vi.mock("@/lib/query/subscription", () => ({
  useAntigravityOauthQuotaByAccountId: mocks.quota,
}));

const accounts = [
  {
    id: "a",
    email: "first@example.test",
    isDefault: true,
    requiresReauth: false,
  },
  {
    id: "b",
    email: "second@example.test",
    isDefault: false,
    requiresReauth: false,
  },
];
function auth() {
  return {
    status: {
      data: { accounts },
      isSuccess: true,
      isPending: false,
      isError: false,
      refetch: vi.fn(),
    },
    busy: false,
    starting: false,
    error: null,
    login: null,
    start: vi.fn(),
    openBrowser: vi.fn(),
    cancel: vi.fn(),
    setDefault: vi.fn(),
    remove: vi.fn(),
  };
}

describe("AntigravityOAuthSection", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocks.auth.mockReturnValue(auth());
    mocks.quota.mockReturnValue({
      data: {
        tool: "antigravity",
        credentialStatus: "valid",
        success: true,
        tiers: [{ name: "new-model", utilization: 37, resetsAt: null }],
      },
      isPending: false,
      isFetching: false,
      refetch: vi.fn(),
    });
  });

  it("renders each account quota including unknown model names", () => {
    render(<AntigravityOAuthSection />);
    expect(mocks.quota).toHaveBeenCalledWith("a");
    expect(mocks.quota).toHaveBeenCalledWith("b");
    expect(screen.getAllByText("new-model")).toHaveLength(2);
    expect(screen.getAllByText("37%")).toHaveLength(2);
  });

  it("renders independent Antigravity model groups and quota windows", () => {
    mocks.quota.mockReturnValue({
      data: {
        tool: "antigravity",
        credentialStatus: "valid",
        success: true,
        tiers: [
          { name: "Gemini Models / 5h", utilization: 18, resetsAt: null },
          { name: "Gemini Models / weekly", utilization: 100, resetsAt: null },
          {
            name: "Claude and GPT models / 5h",
            utilization: 57,
            resetsAt: null,
          },
        ],
      },
      isPending: false,
      isFetching: false,
      refetch: vi.fn(),
    });
    render(<AntigravityOAuthSection />);
    expect(screen.getAllByText("Gemini Models / weekly")).toHaveLength(2);
    expect(screen.getAllByText("Claude and GPT models / 5h")).toHaveLength(2);
    expect(screen.getAllByText("100%")).toHaveLength(2);
  });

  it("targets reauth, default and removal actions to the correct account", () => {
    const state = auth();
    mocks.auth.mockReturnValue(state);
    render(<AntigravityOAuthSection />);
    fireEvent.click(screen.getAllByRole("button", { name: "重新登录" })[1]);
    expect(state.start).toHaveBeenCalledWith("b");
    fireEvent.click(screen.getByRole("button", { name: "设为默认" }));
    expect(state.setDefault).toHaveBeenCalledWith("b");
    fireEvent.click(screen.getAllByRole("button", { name: "移除账号" })[0]);
    expect(state.remove).toHaveBeenCalledWith("a");
    fireEvent.click(
      screen.getByRole("button", { name: "添加其他 Google 账号" }),
    );
    expect(state.start).toHaveBeenLastCalledWith();
  });

  it("shows retryable status errors without offering a misleading empty login", () => {
    const state = auth();
    mocks.auth.mockReturnValue({
      ...state,
      status: {
        ...state.status,
        data: undefined,
        isSuccess: false,
        isError: true,
        error: new Error("storage failed"),
      },
    });
    render(<AntigravityOAuthSection />);
    expect(screen.getByRole("alert")).toHaveTextContent("storage failed");
    expect(
      screen.queryByRole("button", { name: "使用 Google 登录" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect(state.status.refetch).toHaveBeenCalled();
  });

  it("offers reopen and cancel while waiting for Google authorization", () => {
    const state = auth();
    mocks.auth.mockReturnValue({
      ...state,
      busy: true,
      login: { flowId: "pending" },
    });
    render(<AntigravityOAuthSection />);
    fireEvent.click(screen.getByRole("button", { name: "打开授权页" }));
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(state.openBrowser).toHaveBeenCalled();
    expect(state.cancel).toHaveBeenCalled();
    expect(
      screen.getAllByRole("button", { name: "重新登录" })[0],
    ).toBeDisabled();
  });

  it("shows the managed Google reauth hint rather than a CLI login instruction", () => {
    mocks.quota.mockReturnValue({
      data: {
        tool: "antigravity",
        credentialStatus: "expired",
        success: false,
      },
      isPending: false,
      isFetching: false,
      refetch: vi.fn(),
    });
    render(<AntigravityOAuthSection />);
    expect(screen.getAllByText("Google 登录已失效，请重新登录。")).toHaveLength(
      2,
    );
    expect(screen.queryByText(/Gemini CLI.*login/)).not.toBeInTheDocument();
  });

  it("renders quota errors with a manual refresh action", () => {
    const refetch = vi.fn();
    mocks.quota.mockReturnValue({
      data: {
        tool: "antigravity",
        credentialStatus: "valid",
        success: false,
        error: "quota failed",
      },
      isPending: false,
      isFetching: false,
      refetch,
    });
    render(<AntigravityOAuthSection />);
    expect(screen.getAllByText("quota failed")).toHaveLength(2);
    fireEvent.click(screen.getAllByTitle("subscription.refresh")[0]);
    expect(refetch).toHaveBeenCalledTimes(1);
  });
});
