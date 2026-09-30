import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ExternalLink,
  Loader2,
  Plus,
  RefreshCw,
  Star,
  User,
  X,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { SubscriptionQuotaView } from "@/components/SubscriptionQuotaFooter";
import { useAntigravityOauthQuotaByAccountId } from "@/lib/query/subscription";
import { extractErrorMessage } from "@/utils/errorUtils";
import { useAntigravityOauth } from "./hooks/useAntigravityOauth";

function AntigravityAccountQuota({ accountId }: { accountId: string }) {
  const { t } = useTranslation();
  const query = useAntigravityOauthQuotaByAccountId(accountId);
  if (!query.data && query.isPending) {
    return (
      <div
        role="status"
        className="flex items-center gap-2 py-3 text-sm text-muted-foreground"
      >
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("antigravityOauth.loadingQuota", "正在查询配额...")}
      </div>
    );
  }
  return (
    <SubscriptionQuotaView
      quota={query.data}
      loading={query.isFetching}
      refetch={() => void query.refetch()}
      appIdForExpiredHint="Antigravity"
      expiredHint={t(
        "antigravityOauth.expired",
        "Google 登录已失效，请重新登录。",
      )}
      showUnknownTiers
    />
  );
}

export function AntigravityOAuthSection() {
  const { t } = useTranslation();
  const auth = useAntigravityOauth();
  const accounts = auth.status.data?.accounts ?? [];
  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium">
          {t("antigravityOauth.authStatus", "认证状态")}
        </span>
        <Badge
          variant={
            auth.status.isError
              ? "destructive"
              : accounts.length
                ? "default"
                : "secondary"
          }
        >
          {auth.status.isError
            ? t("antigravityOauth.unavailable", "状态不可用")
            : auth.status.isPending
              ? t("antigravityOauth.loading", "正在加载...")
              : accounts.length
                ? t("antigravityOauth.accountCount", {
                    count: accounts.length,
                    defaultValue: `${accounts.length} 个账号`,
                  })
                : t("antigravityOauth.notAuthenticated", "未认证")}
        </Badge>
      </div>
      {auth.status.isPending && (
        <Loader2
          aria-label={t("antigravityOauth.loading", "正在加载...")}
          className="h-4 w-4 animate-spin text-muted-foreground"
        />
      )}
      {auth.status.isError && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-2 text-sm text-destructive"
        >
          <AlertTriangle className="h-4 w-4 shrink-0" />
          <span className="min-w-0 flex-1 break-words">
            {extractErrorMessage(auth.status.error)}
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void auth.status.refetch()}
          >
            <RefreshCw className="mr-1 h-3.5 w-3.5" />
            {t("antigravityOauth.retry", "重试")}
          </Button>
        </div>
      )}
      {auth.status.isSuccess &&
        accounts.map((account) => (
          <div
            key={account.id}
            className="space-y-2 border-b border-border/60 pb-4 last:border-b-0"
          >
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex min-w-0 flex-1 items-center gap-2">
                <User className="h-5 w-5 shrink-0 text-muted-foreground" />
                <span
                  className="min-w-0 break-all text-sm font-medium"
                  title={account.email}
                >
                  {account.email}
                </span>
                {account.isDefault && (
                  <Badge variant="secondary" className="shrink-0">
                    {t("antigravityOauth.default", "默认")}
                  </Badge>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <Button
                  variant="outline"
                  size="sm"
                  className="h-7 gap-1 px-2 text-xs"
                  disabled={auth.busy}
                  onClick={() => void auth.start(account.id)}
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("antigravityOauth.reauth", "重新登录")}
                </Button>
                {!account.isDefault && (
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7"
                    disabled={auth.busy}
                    title={t("antigravityOauth.setDefault", "设为默认")}
                    aria-label={t("antigravityOauth.setDefault", "设为默认")}
                    onClick={() => auth.setDefault(account.id)}
                  >
                    <Star className="h-4 w-4" />
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 text-muted-foreground hover:text-red-500"
                  disabled={auth.busy}
                  title={t("antigravityOauth.remove", "移除账号")}
                  aria-label={t("antigravityOauth.remove", "移除账号")}
                  onClick={() => auth.remove(account.id)}
                >
                  <X className="h-4 w-4" />
                </Button>
              </div>
            </div>
            {account.requiresReauth ? (
              <p
                role="alert"
                className="flex items-center gap-2 text-sm text-amber-600 dark:text-amber-400"
              >
                <AlertTriangle className="h-4 w-4 shrink-0" />
                {t(
                  "antigravityOauth.expired",
                  "Google 登录已失效，请重新登录。",
                )}
              </p>
            ) : (
              <AntigravityAccountQuota accountId={account.id} />
            )}
          </div>
        ))}
      {auth.error && (
        <p role="alert" className="break-words text-sm text-destructive">
          {auth.error}
        </p>
      )}
      {auth.login ? (
        <div className="flex flex-wrap items-center gap-2 border-t border-border/60 pt-3">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          <span className="min-w-0 flex-1 text-sm text-muted-foreground">
            {t("antigravityOauth.waiting", "等待 Google 授权...")}
          </span>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void auth.openBrowser()}
          >
            <ExternalLink className="mr-1 h-3.5 w-3.5" />
            {t("antigravityOauth.openBrowser", "打开授权页")}
          </Button>
          <Button variant="ghost" size="sm" onClick={() => void auth.cancel()}>
            {t("common.cancel", "取消")}
          </Button>
        </div>
      ) : (
        auth.status.isSuccess && (
          <Button
            variant="outline"
            className="w-full"
            disabled={auth.busy}
            onClick={() => void auth.start()}
          >
            {auth.starting ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Plus className="mr-2 h-4 w-4" />
            )}
            {accounts.length
              ? t("antigravityOauth.addAccount", "添加其他 Google 账号")
              : t("antigravityOauth.login", "使用 Google 登录")}
          </Button>
        )
      )}
    </div>
  );
}
