import React from "react";
import { useCodexOauthQuotaByAccountId } from "@/lib/query/subscription";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";

interface CodexOauthAccountQuotaProps {
  /** cc-switch 自管的 ChatGPT 账号 ID */
  accountId: string;
  /** 账号名，拼进刷新按钮的无障碍名字 */
  login?: string;
}

/**
 * 授权中心里单个 ChatGPT (Codex OAuth) 账号的订阅额度：展开式现代化全宽卡片（与 Antigravity 统一布局）。
 */
const CodexOauthAccountQuota: React.FC<CodexOauthAccountQuotaProps> = ({
  accountId,
}) => {
  const {
    data: quota,
    isFetching: loading,
    refetch,
  } = useCodexOauthQuotaByAccountId(accountId, {
    enabled: true,
    autoQuery: false,
  });

  return (
    <div data-testid="account-quota">
      <SubscriptionQuotaView
        quota={quota}
        loading={loading}
        refetch={() => void refetch()}
        appIdForExpiredHint="ChatGPT"
        showUnknownTiers
      />
    </div>
  );
};

export default CodexOauthAccountQuota;
