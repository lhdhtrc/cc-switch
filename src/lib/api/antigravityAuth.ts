import { invoke } from "@tauri-apps/api/core";
// Managed Antigravity accounts are independent of Gemini CLI credentials.
import type { SubscriptionQuota } from "@/types/subscription";

export interface AntigravityAccount {
  id: string;
  email: string;
  authenticatedAt: number;
  isDefault: boolean;
  requiresReauth: boolean;
}

export interface AntigravityAuthStatus {
  accounts: AntigravityAccount[];
  defaultAccountId: string | null;
}

export interface AntigravityLogin {
  flowId: string;
  authorizationUrl: string;
  expiresAt: number;
}

export type AntigravityLoginResult =
  | { status: "pending" }
  | { status: "completed"; account: AntigravityAccount }
  | { status: "error"; message: string };

export const antigravityAuthApi = {
  status: (): Promise<AntigravityAuthStatus> =>
    invoke("antigravity_auth_status"),
  start: (targetAccountId: string | null): Promise<AntigravityLogin> =>
    invoke("antigravity_auth_start", { targetAccountId }),
  poll: (flowId: string): Promise<AntigravityLoginResult> =>
    invoke("antigravity_auth_poll", { flowId }),
  cancel: (flowId: string): Promise<void> =>
    invoke("antigravity_auth_cancel", { flowId }),
  remove: (accountId: string): Promise<void> =>
    invoke("antigravity_auth_remove", { accountId }),
  setDefault: (accountId: string): Promise<void> =>
    invoke("antigravity_auth_set_default", { accountId }),
  quota: (accountId: string): Promise<SubscriptionQuota> =>
    invoke("get_antigravity_oauth_quota", { accountId }),
};
