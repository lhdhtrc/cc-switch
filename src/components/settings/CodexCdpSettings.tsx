import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Loader2, Unlock, Monitor, CheckCircle2, XCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { proxyApi } from "@/lib/api";
import { codexCdpStatusView, type CdpStatusLevel } from "@/lib/cdpStatus";

export function CodexCdpSettings() {
  const { t } = useTranslation();
  const [cdpUnlocking, setCdpUnlocking] = useState(false);
  const [cdpChecking, setCdpChecking] = useState(false);
  const [cdpStatus, setCdpStatus] = useState<string | null>(null);
  const [statusType, setStatusType] = useState<CdpStatusLevel | null>(null);

  const handleCheckCdpStatus = async () => {
    setCdpChecking(true);
    setStatusType(null);
    try {
      const status = await proxyApi.checkCodexCdpStatus();
      const view = codexCdpStatusView(status, t);
      setCdpStatus(view.text);
      setStatusType(view.level);
      if (view.level === "ok") {
        toast.success(view.text);
      } else if (view.level === "warn") {
        toast.warning(view.text);
      } else {
        toast.error(view.text);
      }
    } catch (e) {
      toast.error(String(e));
      setStatusType("err");
    } finally {
      setCdpChecking(false);
    }
  };

  const handleCdpUnlock = async () => {
    setCdpUnlocking(true);
    setStatusType(null);
    try {
      const result = await proxyApi.unlockCodexReasoningEffort();
      if (result.state === "injected") {
        const message = t("codex.unlockInjected", {
          targets: result.injectedTargets,
          models: result.modelCount,
          defaultValue:
            "思考强度已解锁：已注入 {{targets}} 个 Codex 页面（共 {{models}} 个模型）。",
        });
        setCdpStatus(message);
        setStatusType("ok");
        toast.success(message);
      } else {
        setCdpStatus(result.message);
        setStatusType("err");
        toast.error(result.message);
      }
    } catch (e) {
      const message = String(e);
      if (message.includes("without CDP")) {
        const hint = t("codex.cdpStatusRunningWithoutCdp", {
          defaultValue:
            "Codex Desktop 正在运行但未开启 CDP。请先完全退出 Codex Desktop，再点击「解锁思考强度」，CC Switch 会以调试端口重新启动 Codex 并注入补丁。",
        });
        setCdpStatus(hint);
        setStatusType("err");
        toast.warning(hint);
      } else {
        toast.error(message);
        setStatusType("err");
      }
    } finally {
      setCdpUnlocking(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="text-sm text-muted-foreground">
        {t("codex.cdpDescription", {
          defaultValue:
            "CDP (Chrome DevTools Protocol) 用于连接 Codex Desktop 渲染进程，为非 GPT 模型解锁「思考强度」下拉选项。",
        })}
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="outline"
          size="sm"
          onClick={handleCheckCdpStatus}
          disabled={cdpChecking}
          className="text-xs"
        >
          {cdpChecking ? (
            <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
          ) : (
            <Monitor className="mr-1.5 h-3.5 w-3.5" />
          )}
          {t("codex.checkCdp", { defaultValue: "检查 CDP 状态" })}
        </Button>

        <Button
          variant="outline"
          size="sm"
          onClick={handleCdpUnlock}
          disabled={cdpUnlocking}
          className="text-xs"
        >
          {cdpUnlocking ? (
            <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
          ) : (
            <Unlock className="mr-1.5 h-3.5 w-3.5" />
          )}
          {t("codex.unlockModelPicker", { defaultValue: "解锁思考强度" })}
        </Button>
      </div>

      {cdpStatus && (
        <div
          className={
            statusType === "ok"
              ? "flex items-center gap-2 text-sm text-green-600 dark:text-green-400"
              : statusType === "warn"
                ? "flex items-center gap-2 text-sm text-amber-600 dark:text-amber-400"
                : "flex items-center gap-2 text-sm text-red-600 dark:text-red-400"
          }
        >
          {statusType === "ok" || statusType === "warn" ? (
            <CheckCircle2 className="h-4 w-4" />
          ) : (
            <XCircle className="h-4 w-4" />
          )}
          <span>{cdpStatus}</span>
        </div>
      )}

      <div className="rounded-md border bg-muted/30 p-3 text-xs text-muted-foreground space-y-1.5">
        <p className="font-medium text-foreground">
          {t("codex.howToUse", { defaultValue: "使用说明" })}
        </p>
        <ol className="list-decimal pl-4 space-y-1">
          <li>
            {t("codex.step1", {
              defaultValue: "若 Codex Desktop 正在运行，先将其完全退出",
            })}
          </li>
          <li>
            {t("codex.step2", {
              defaultValue: "点击「检查 CDP 状态」查看当前状态",
            })}
          </li>
          <li>
            {t("codex.step3", {
              defaultValue:
                "点击「解锁思考强度」自动注入 JS 补丁，所有模型将显示 reasoning effort 下拉",
            })}
          </li>
        </ol>
        <p className="pt-1 text-muted-foreground">
          {t("codex.note", {
            defaultValue:
              "提示：解锁后请保持 Codex Desktop 开启；重启 Codex 后需要重新解锁。",
          })}
        </p>
      </div>
    </div>
  );
}
