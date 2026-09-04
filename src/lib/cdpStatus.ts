import type { CodexCdpStatus } from "@/lib/api/proxy";

type Translate = (key: string, options?: Record<string, unknown>) => string;

export type CdpStatusLevel = "ok" | "warn" | "err";

export interface CdpStatusView {
  text: string;
  level: CdpStatusLevel;
}

export function codexCdpStatusView(
  status: CodexCdpStatus,
  t: Translate,
): CdpStatusView {
  switch (status.state) {
    case "cdp_ready":
      return {
        text: t("codex.cdpStatusReady", {
          port: status.port,
          defaultValue:
            "Codex Desktop 已开启 CDP（端口 {{port}}），思考强度已可解锁。",
        }),
        level: "ok",
      };
    case "running_without_cdp":
      return {
        text: t("codex.cdpStatusRunningWithoutCdp", {
          defaultValue:
            "Codex Desktop 正在运行但未开启 CDP。请先完全退出 Codex Desktop，再点击「解锁思考强度」，CC Switch 会以调试端口重新启动 Codex 并注入补丁。",
        }),
        level: "err",
      };
    case "not_running":
      return {
        text: t("codex.cdpStatusNotRunning", {
          path: status.executable,
          defaultValue:
            "Codex Desktop 未运行（已定位到 {{path}}）。点击「解锁思考强度」将以调试模式启动 Codex 并注入补丁。",
        }),
        level: "warn",
      };
    case "not_found":
      return {
        text: t("codex.cdpStatusNotFound", {
          defaultValue:
            "未找到 Codex Desktop。请先安装并启动一次 Codex Desktop，再使用本功能。",
        }),
        level: "err",
      };
    default:
      return { text: status.message, level: "err" };
  }
}
