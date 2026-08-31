import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { providersApi } from "@/lib/api";

interface CodexAggregationPageProps {
  onClose: () => void;
}

/**
 * Codex 多中转聚合页面（独立于单供应商模式）
 *
 * - 聚合模式开关（两种模式互斥：单供应商 / 聚合）；
 * - 选择参与聚合的供应商，可拉取/查看各供应商模型列表；
 * - 聚合模型视图：合并展示所有参与供应商的模型，同名模型可指定来源中转；
 * - 应用后把合并目录写入 Codex live 配置，代理按模型路由到对应中转。
 */
export function CodexAggregationPage({ onClose }: CodexAggregationPageProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [fetchingId, setFetchingId] = useState<string | null>(null);
  const [fetchedModels, setFetchedModels] = useState<Record<string, string[]>>(
    {},
  );

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
        const entry = map.get(m) ?? { model: m, owners: [] };
        entry.owners.push(p.id);
        map.set(m, entry);
      }
    }
    return [...map.values()];
  }, [enabledProviders]);

  const providerName = (id: string) =>
    data?.providers.find((p) => p.id === id)?.name ?? id;

  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ["codex", "aggregation"] });
  };

  const toggleEnabled = async (enabled: boolean) => {
    try {
      await providersApi.setCodexAggregationEnabled(enabled);
      refresh();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const toggleProvider = async (id: string, enabled: boolean) => {
    try {
      await providersApi.setCodexAggregationProvider(id, enabled);
      refresh();
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
      // 拉取结果已写入供应商 modelCatalog，刷新让模型徽章渲染出来
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
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-6">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-lg font-semibold">
            {t("aggregation.title", { defaultValue: "Codex 多中转聚合" })}
          </h1>
          <p className="text-sm text-muted-foreground">
            {t("aggregation.hint", {
              defaultValue:
                "独立于单供应商模式：启用多个中转后模型目录合并展示，请求按模型自动路由到对应中转；模型详情配置仍在其各自供应商中。",
            })}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-3">
          <label className="flex items-center gap-2 text-sm">
            {t("aggregation.mode", { defaultValue: "聚合模式" })}
            <Switch
              checked={data?.enabled ?? false}
              onCheckedChange={toggleEnabled}
            />
          </label>
          <Button variant="ghost" size="sm" onClick={onClose}>
            {t("common.close", { defaultValue: "关闭" })}
          </Button>
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">
            {t("aggregation.providers", {
              defaultValue: "参与聚合的供应商",
            })}
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {data?.providers.map((p) => (
            <div
              key={p.id}
              className="flex items-start justify-between gap-4 rounded-md border p-3"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{p.name}</span>
                  <Switch
                    checked={p.enabled}
                    onCheckedChange={(v) => toggleProvider(p.id, v)}
                  />
                </div>
                <div className="mt-1 flex flex-wrap gap-1">
                  {p.models.map((m) => (
                    <span
                      key={m}
                      className="rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground"
                    >
                      {m}
                    </span>
                  ))}
                  {p.models.length === 0 && (
                    <span className="text-xs text-muted-foreground">
                      {noModelsHint}
                    </span>
                  )}
                </div>
                <div className="mt-2">
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
            </div>
          ))}
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
        <CardHeader>
          <CardTitle className="text-sm">
            {t("aggregation.merged", { defaultValue: "聚合模型" })}
          </CardTitle>
        </CardHeader>
        <CardContent>
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
      </Card>

      <div className="flex justify-end">
        <Button
          onClick={apply}
          disabled={!data?.enabled || enabledProviders.length === 0}
        >
          {t("aggregation.apply", {
            defaultValue: "应用并写入 Codex 配置",
          })}
        </Button>
      </div>
    </div>
  );
}
