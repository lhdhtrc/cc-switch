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
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { providersApi } from "@/lib/api";
import { cn } from "@/lib/utils";

interface CodexAggregationPageProps {}

const DEFAULT_WEIGHT = 100;

/**
 * Codex 多中转聚合页面（独立于单供应商模式）
 *
 * - 聚合模式开关（两种模式互斥：单供应商 / 聚合）；
 * - 选择参与聚合的供应商，可拉取/查看各供应商模型列表（可折叠）；
 * - 聚合模型视图：合并展示所有参与供应商的模型（可折叠），同名模型可指定来源中转；
 * - 应用后把合并目录写入 Codex live 配置，代理按模型路由到对应中转。
 */
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
  const [weightDrafts, setWeightDrafts] = useState<Record<string, string>>({});
  // 聚合模型区折叠状态（默认展开）
  const [mergedCollapsed, setMergedCollapsed] = useState(false);

  const { data } = useQuery({
    queryKey: ["codex", "aggregation"],
    queryFn: () => providersApi.getCodexAggregationConfig(),
  });

  const enabledProviders = useMemo(
    () =>
      (data?.providers ?? [])
        .filter((p) => p.enabled)
        .sort(
          (a, b) =>
            (b.weight ?? DEFAULT_WEIGHT) - (a.weight ?? DEFAULT_WEIGHT),
        ),
    [data],
  );

  const providerRows = useMemo(() => {
    const rows = [...(data?.providers ?? [])];
    rows.sort(
      (a, b) =>
        Number(b.enabled) - Number(a.enabled) ||
        (b.weight ?? DEFAULT_WEIGHT) - (a.weight ?? DEFAULT_WEIGHT) ||
        a.name.localeCompare(b.name),
    );
    return rows;
  }, [data]);

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
      if (!enabled) {
        setWeightDrafts((prev) => {
          const next = { ...prev };
          delete next[id];
          return next;
        });
      }
      refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const commitWeight = async (id: string, raw: string, current: number) => {
    const next = Number(raw);
    if (!Number.isInteger(next) || next <= 0) {
      toast.error(
        t("aggregation.invalidWeight", {
          defaultValue: "权重必须是大于 0 的整数",
        }),
      );
      setWeightDrafts((prev) => ({ ...prev, [id]: String(current) }));
      return;
    }
    try {
      await providersApi.setCodexAggregationProviderWeight(id, next);
      setWeightDrafts((prev) => ({ ...prev, [id]: String(next) }));
      toast.success(
        t("aggregation.weightSet", {
          defaultValue: `权重已设为 ${next}`,
        }),
      );
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

  const setDefaultModel = async (model: string | null) => {
    try {
      await providersApi.setCodexAggregationDefaultModel(model);
      toast.success(
        model
          ? t("aggregation.defaultModelSet", {
              defaultValue: `默认模型已设为 ${model}`,
            })
          : t("aggregation.defaultModelCleared", {
              defaultValue: "已清除默认模型，回退聚合目录默认模型",
            }),
      );
      refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const setBinding = async (model: string, providerId: string | null) => {
    try {
      await providersApi.setCodexAggregationBinding(model, providerId);
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
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-4 pb-6">
      <div className="rounded-md border px-4 py-2 text-xs text-muted-foreground">
        {data?.enabled
          ? t("aggregation.modeHintOn", {
              defaultValue:
                "当前为聚合模式：代理接管全部 Codex 内容，模型目录为合并目录，请求按模型路由到对应中转；默认模型与子 agent 配置由聚合合成骨架统一提供（spawn_agent 选择器跟随会话默认模型）。切换模式即写入 Codex 配置。",
            })
          : t("aggregation.modeHintOff", {
              defaultValue:
                "当前为单供应商模式：仅使用活跃供应商的模型目录（仍经本地代理做 Chat 转换分流）。切换模式即写入 Codex 配置。",
            })}
      </div>
      <Card>
        <CardHeader className="px-4 pb-1.5 pt-3">
          <CardTitle className="text-sm">
            {t("aggregation.providers", {
              defaultValue: "参与聚合的供应商",
            })}
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 px-4 pb-4">
          {providerRows.map((p) => {
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
                    <label className="text-xs text-muted-foreground">
                      {t("aggregation.weight", { defaultValue: "权重" })}
                    </label>
                    <Input
                      type="number"
                      min={1}
                      max={9999}
                      disabled={!p.enabled}
                      className="h-7 w-20 px-2 text-xs"
                      value={weightDrafts[p.id] ?? String(p.weight ?? DEFAULT_WEIGHT)}
                      onChange={(e) =>
                        setWeightDrafts((prev) => ({
                          ...prev,
                          [p.id]: e.target.value,
                        }))
                      }
                      onBlur={(e) =>
                        commitWeight(p.id, e.target.value, p.weight ?? DEFAULT_WEIGHT)
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.currentTarget.blur();
                        }
                      }}
                    />
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
                      <th className="py-2 pl-4 text-right font-medium">
                        {t("aggregation.colDefault", { defaultValue: "默认" })}
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
                          {owners.length > 1 ? (
                            <Select
                              value={data?.bindings[model] ?? "auto"}
                              onValueChange={(v) =>
                                setBinding(model, v === "auto" ? null : v)
                              }
                            >
                              <SelectTrigger className="h-7 w-40 text-xs">
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value="auto">
                                  {t("aggregation.bindingAuto", {
                                    defaultValue: "自动",
                                  })}
                                </SelectItem>
                                {owners.map((id) => (
                                  <SelectItem key={id} value={id}>
                                    {providerName(id)}
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          ) : (
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
                          )}
                        </td>
                        <td className="py-2 pl-4 align-top">
                          <div className="flex justify-end">
                            <Switch
                              checked={data?.defaultModel === model}
                              onCheckedChange={(v) =>
                                setDefaultModel(v ? model : null)
                              }
                              aria-label={t("aggregation.setDefault", {
                                defaultValue: "设为默认模型",
                              })}
                            />
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
