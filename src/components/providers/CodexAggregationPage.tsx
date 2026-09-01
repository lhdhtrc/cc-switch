import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ChevronDown, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Switch } from "@/components/ui/switch";
import { providersApi } from "@/lib/api";
import { cn } from "@/lib/utils";

interface CodexAggregationPageProps {}

/**
 * Codex 多中转聚合页面（独立于单供应商模式）
 *
 * - 聚合模式开关（两种模式互斥：单供应商 / 聚合）；
 * - 选择参与聚合的供应商，可拉取/查看各供应商模型列表（可折叠）；
 * - 聚合模型视图：合并展示所有参与供应商的模型（可折叠），同名模型可指定来源中转；
 * - 应用后把合并目录写入 Codex live 配置，代理按模型路由到对应中转。
 */
/** 聚合页顶部操作：聚合模式开关 + 应用按钮（放在标题栏右侧） */
export function CodexAggregationHeaderActions() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data } = useQuery({
    queryKey: ["codex", "aggregation"],
    queryFn: () => providersApi.getCodexAggregationConfig(),
  });
  const enabledCount = (data?.providers ?? []).filter((p) => p.enabled).length;

  const toggle = async (enabled: boolean) => {
    try {
      await providersApi.setCodexAggregationEnabled(enabled);
      queryClient.invalidateQueries({ queryKey: ["codex", "aggregation"] });
    } catch (e) {
      toast.error(String(e));
    }
  };

  const apply = async () => {
    try {
      await providersApi.applyCodexAggregation();
      toast.success(
        t("aggregation.applied", { defaultValue: "已写入 Codex 配置" }),
      );
    } catch (e) {
      toast.error(String(e));
    }
  };

  return (
    <div className="flex items-center gap-2">
      <label className="flex items-center gap-1.5 text-xs text-muted-foreground">
        {t("aggregation.mode", { defaultValue: "聚合模式" })}
        <Switch checked={data?.enabled ?? false} onCheckedChange={toggle} />
      </label>
      <Button
        size="sm"
        onClick={apply}
        disabled={!data?.enabled || enabledCount === 0}
      >
        {t("aggregation.applyShort", { defaultValue: "应用" })}
      </Button>
    </div>
  );
}

export function CodexAggregationPage(_props: CodexAggregationPageProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [fetchingId, setFetchingId] = useState<string | null>(null);
  const [fetchedModels, setFetchedModels] = useState<Record<string, string[]>>(
    {},
  );
  // 各供应商模型列表折叠状态（默认展开）
  const [collapsedProviders, setCollapsedProviders] = useState<
    Record<string, boolean>
  >({});
  // 聚合模型区折叠状态（默认展开）
  const [mergedCollapsed, setMergedCollapsed] = useState(false);

  const { data } = useQuery({
    queryKey: ["codex", "aggregation"],
    queryFn: () => providersApi.getCodexAggregationConfig(),
  });

  const enabledProviders = useMemo(
    () => (data?.providers ?? []).filter((p) => p.enabled),
    [data],
  );

  // 合并模型视图：model -> 拥有该模型的供应商列表
  const merged = useMemo(() => {
    const map = new Map<string, { model: string; owners: string[] }>();
    for (const p of enabledProviders) {
      for (const m of p.models) {
        if (m.hidden) continue;
        const id = m.id;
        const entry = map.get(id) ?? { model: id, owners: [] };
        entry.owners.push(p.id);
        map.set(id, entry);
      }
    }
    return [...map.values()];
  }, [enabledProviders]);

  const providerName = (id: string) =>
    data?.providers.find((p) => p.id === id)?.name ?? id;

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ["codex", "aggregation"] });
  };

  const toggleProvider = async (id: string, enabled: boolean) => {
    try {
      await providersApi.setCodexAggregationProvider(id, enabled);
      refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const toggleProviderCollapsed = (id: string, open: boolean) => {
    setCollapsedProviders((prev) => ({ ...prev, [id]: !open }));
  };

  const toggleHidden = async (
    providerId: string,
    model: string,
    hidden: boolean,
  ) => {
    try {
      await providersApi.setCodexAggregationModelHidden(
        providerId,
        model,
        hidden,
      );
      refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const fetchModels = async (id: string) => {
    setFetchingId(id);
    try {
      const models = await providersApi.fetchCodexAggregationModels(id);
      setFetchedModels((prev) => ({ ...prev, [id]: models }));
      toast.success(
        t("aggregation.fetched", {
          defaultValue: `已拉取并保存 ${models.length} 个模型`,
        }),
      );
      // 拉取结果已写入供应商 modelCatalog，刷新让模型渲染出来
      queryClient.invalidateQueries({ queryKey: ["codex", "aggregation"] });
    } catch (e) {
      toast.error(String(e));
    } finally {
      setFetchingId(null);
    }
  };

  const noModelsHint = t("aggregation.noModels", {
    defaultValue: "暂无模型（可在供应商编辑中配置，或点击下方拉取）",
  });

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 px-6 pb-6 pt-2">
      <Card>
        <CardHeader className="px-4 pb-1.5 pt-3">
          <CardTitle className="text-sm">
            {t("aggregation.providers", {
              defaultValue: "参与聚合的供应商",
            })}
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 px-4 pb-4">
          {data?.providers.map((p) => {
            const collapsed = collapsedProviders[p.id] ?? false;
            return (
              <div key={p.id} className="rounded-md border">
                <div className="flex items-center justify-between gap-3 px-3 py-2">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="font-medium">{p.name}</span>
                    <Switch
                      checked={p.enabled}
                      onCheckedChange={(v) => toggleProvider(p.id, v)}
                    />
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <span className="text-xs text-muted-foreground">
                      {p.models.length} 个模型
                    </span>
                    <Collapsible
                      open={!collapsed}
                      onOpenChange={(open) =>
                        toggleProviderCollapsed(p.id, open)
                      }
                    >
                      <CollapsibleTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7"
                          aria-label={
                            collapsed
                              ? t("aggregation.expand", {
                                  defaultValue: "展开",
                                })
                              : t("aggregation.collapse", {
                                  defaultValue: "折叠",
                                })
                          }
                        >
                          {collapsed ? (
                            <ChevronRight className="h-4 w-4" />
                          ) : (
                            <ChevronDown className="h-4 w-4" />
                          )}
                        </Button>
                      </CollapsibleTrigger>
                    </Collapsible>
                  </div>
                </div>
                <Collapsible
                  open={!collapsed}
                  onOpenChange={(open) => toggleProviderCollapsed(p.id, open)}
                >
                  <CollapsibleContent>
                    <div className="space-y-2 border-t px-3 py-2">
                      {p.models.map((m) => (
                        <div
                          key={m.id}
                          className="flex items-center justify-between gap-2 rounded bg-muted/50 px-2 py-1"
                        >
                          <span
                            className={cn(
                              "truncate text-xs",
                              m.hidden && "text-muted-foreground line-through",
                            )}
                          >
                            {m.id}
                          </span>
                          <div className="flex shrink-0 items-center gap-1.5">
                            <span className="text-[10px] text-muted-foreground">
                              {m.hidden
                                ? t("aggregation.hidden", {
                                    defaultValue: "已隐藏",
                                  })
                                : t("aggregation.visible", {
                                    defaultValue: "显示",
                                  })}
                            </span>
                            <Switch
                              checked={!m.hidden}
                              onCheckedChange={(v) =>
                                toggleHidden(p.id, m.id, !v)
                              }
                              aria-label={m.id}
                            />
                          </div>
                        </div>
                      ))}
                      {p.models.length === 0 && (
                        <span className="text-xs text-muted-foreground">
                          {noModelsHint}
                        </span>
                      )}
                      <div className="pt-1">
                        <Button
                          size="sm"
                          variant="outline"
                          onClick={() => fetchModels(p.id)}
                          disabled={fetchingId === p.id}
                        >
                          {fetchingId === p.id
                            ? t("aggregation.fetching", {
                                defaultValue: "拉取中...",
                              })
                            : t("aggregation.fetch", {
                                defaultValue: "拉取模型列表",
                              })}
                        </Button>
                        {fetchedModels[p.id] && (
                          <span className="ml-2 text-xs text-muted-foreground">
                            {t("aggregation.fetchedCount", {
                              defaultValue: `远端 ${fetchedModels[p.id].length} 个`,
                            })}
                          </span>
                        )}
                      </div>
                    </div>
                  </CollapsibleContent>
                </Collapsible>
              </div>
            );
          })}
          {(data?.providers.length ?? 0) === 0 && (
            <p className="text-sm text-muted-foreground">
              {t("aggregation.empty", {
                defaultValue: "尚未添加 Codex 供应商",
              })}
            </p>
          )}
        </CardContent>
      </Card>

      <Card>
        <Collapsible
          open={!mergedCollapsed}
          onOpenChange={(open) => setMergedCollapsed(!open)}
        >
          <CollapsibleTrigger asChild>
            <CardHeader
              className={cn(
                "cursor-pointer select-none px-4 pt-3",
                mergedCollapsed ? "pb-3" : "pb-1.5",
              )}
            >
              <div className="flex items-center justify-between">
                <CardTitle className="text-sm">
                  {t("aggregation.merged", { defaultValue: "聚合模型" })}
                </CardTitle>
                {mergedCollapsed ? (
                  <ChevronRight className="h-4 w-4 text-muted-foreground" />
                ) : (
                  <ChevronDown className="h-4 w-4 text-muted-foreground" />
                )}
              </div>
            </CardHeader>
          </CollapsibleTrigger>
          <CollapsibleContent>
            <CardContent className="px-4 pb-4">
              {merged.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  {t("aggregation.mergedEmpty", {
                    defaultValue:
                      "开启聚合模式并启用至少一个供应商后，这里会显示合并的模型列表。",
                  })}
                </p>
              ) : (
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b text-left text-xs text-muted-foreground">
                      <th className="w-1/2 py-2 pr-4 font-medium">
                        {t("aggregation.colModel", { defaultValue: "模型" })}
                      </th>
                      <th className="py-2 font-medium">
                        {t("aggregation.colSource", { defaultValue: "来源" })}
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y">
                    {merged.map(({ model, owners }) => (
                      <tr key={model}>
                        <td className="py-2 pr-4 align-top">
                          <div className="flex flex-wrap items-center gap-2">
                            <code className="text-sm">{model}</code>
                            {owners.length > 1 && (
                              <span className="rounded bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-700 dark:bg-amber-900/40 dark:text-amber-300">
                                {t("aggregation.duplicate", {
                                  defaultValue: "多中转同名",
                                })}
                              </span>
                            )}
                          </div>
                        </td>
                        <td className="py-2 align-top">
                          <div className="flex flex-wrap gap-1">
                            {owners.map((id) => (
                              <span
                                key={id}
                                className="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
                              >
                                {providerName(id)}
                              </span>
                            ))}
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </CardContent>
          </CollapsibleContent>
        </Collapsible>
      </Card>
    </div>
  );
}
