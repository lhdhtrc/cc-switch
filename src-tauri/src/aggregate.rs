//! 多中转聚合模块（Codex）
//!
//! 目标：让多个中转供应商同时启用，模型目录聚合展示，代理按模型路由到
//! 提供该模型的中转。设计采用「物化聚合」：
//!
//! - 每个 Codex 供应商可通过 `meta.aggregateEnabled` 参与聚合；
//! - 活跃供应商的 `settings_config.modelCatalog` 会被合并成聚合目录
//!   （`apply_codex_aggregation`），每条目写入 `providerId` 记录来源中转；
//! - 同名模型去重：默认聚合顺序（活跃优先）决定来源，也可通过活跃供应商
//!   `meta.aggregateModelBindings`（model -> providerId）显式指定；
//! - 代理收到请求后按 `request_model` 解析目标中转并路由（`resolve_codex_model_provider`）。

use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::HashSet;

/// 供应商是否参与 Codex 多中转聚合。
pub fn aggregate_enabled(provider: &Provider) -> bool {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.aggregate_enabled)
        .unwrap_or(false)
}

/// 读取模型目录条目里的来源中转 id（聚合时写入）。
fn entry_provider_id(entry: &Value) -> Option<String> {
    entry
        .get("providerId")
        .or_else(|| entry.get("provider_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 模型目录中是否存在某个模型 id。
pub fn provider_has_model(provider: &Provider, model: &str) -> bool {
    provider
        .settings_config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(|models| models.as_array())
        .map(|models| {
            models
                .iter()
                .any(|entry| entry.get("model").and_then(|value| value.as_str()) == Some(model))
        })
        .unwrap_or(false)
}

/// 取模型目录中某模型条目。
fn catalog_entry<'a>(provider: &'a Provider, model: &str) -> Option<&'a Value> {
    provider
        .settings_config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(|models| models.as_array())
        .and_then(|models| {
            models
                .iter()
                .find(|entry| entry.get("model").and_then(|value| value.as_str()) == Some(model))
        })
}

/// 该模型是否有显式绑定（存在绑定即强制路由到绑定中转）。
pub fn codex_model_has_binding(
    model: &str,
    active_id: &str,
    all: &IndexMap<String, Provider>,
) -> bool {
    all.get(active_id)
        .and_then(|active| active.meta.as_ref())
        .map(|meta| meta.aggregate_model_bindings.contains_key(model))
        .unwrap_or(false)
}

/// 解析某个模型应路由到哪个中转。
///
/// 优先级：
/// 1. 活跃供应商 `aggregateModelBindings[model]` 的显式绑定；
/// 2. 活跃供应商聚合目录条目上的 `providerId`（物化聚合结果）；
/// 3. 按聚合顺序（活跃优先，其余按列表顺序）找第一个提供该模型的中转。
pub fn resolve_codex_model_provider<'a>(
    model: &str,
    active_id: &str,
    all: &'a IndexMap<String, Provider>,
) -> Option<&'a Provider> {
    let active = all.get(active_id);

    // 1) 显式绑定
    if let Some(active) = active {
        if let Some(bound) = active
            .meta
            .as_ref()
            .and_then(|meta| meta.aggregate_model_bindings.get(model))
        {
            if let Some(provider) = all.get(bound) {
                return Some(provider);
            }
        }
    }

    // 2) 活跃供应商聚合目录条目上的 providerId
    if let Some(active) = active {
        if let Some(entry) = catalog_entry(active, model) {
            if let Some(provider_id) = entry_provider_id(entry) {
                if let Some(provider) = all.get(&provider_id) {
                    return Some(provider);
                }
            }
        }
    }

    // 3) 聚合顺序查找
    if let Some(active) = active {
        if provider_has_model(active, model) {
            return Some(active);
        }
    }
    all.values().find(|provider| {
        provider.id != active_id
            && aggregate_enabled(provider)
            && provider_has_model(provider, model)
    })
}

/// 聚合顺序：活跃供应商优先，其余按存储顺序。
fn aggregation_order<'a>(
    active: &'a Provider,
    all: &'a IndexMap<String, Provider>,
) -> Vec<&'a Provider> {
    let mut order = Vec::new();
    order.push(active);
    for provider in all.values() {
        if provider.id != active.id && aggregate_enabled(provider) {
            order.push(provider);
        }
    }
    order
}

/// 合并所有参与聚合的中转的模型目录（幂等）。
///
/// - 去重按 `model` id，首个出现的条目胜出；
/// - 活跃供应商目录中带 `providerId` 的条目（上一次物化结果）归属对应中转，
///   不会被重新归属到活跃供应商；
/// - `aggregateModelBindings` 指定 model -> providerId 时，只允许该中转的条目胜出；
/// - 每个条目写入 `providerId` 标注来源中转。
pub fn merge_codex_model_catalog(active: &Provider, all: &IndexMap<String, Provider>) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let bindings = active
        .meta
        .as_ref()
        .map(|meta| meta.aggregate_model_bindings.clone())
        .unwrap_or_default();

    for provider in aggregation_order(active, all) {
        let Some(models) = provider
            .settings_config
            .get("modelCatalog")
            .and_then(|catalog| catalog.get("models"))
            .and_then(|models| models.as_array())
            .cloned()
        else {
            continue;
        };

        for mut entry in models {
            let Some(model_id) = entry
                .get("model")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
            else {
                continue;
            };

            if seen.contains(&model_id) {
                continue;
            }

            // 条目归属：已有 providerId 的（物化残留）按原归属，否则属于当前中转。
            let owner = entry_provider_id(&entry).unwrap_or_else(|| provider.id.clone());
            if owner != provider.id {
                continue;
            }

            // 显式绑定：只允许绑定中转的条目胜出。
            if let Some(bound) = bindings.get(&model_id) {
                if bound != &provider.id {
                    continue;
                }
            }

            entry["providerId"] = json!(provider.id);
            merged.push(entry);
            seen.insert(model_id);
        }
    }

    json!({ "models": merged })
}

/// 将聚合目录物化写入活跃供应商的 `settings_config.modelCatalog`。
pub fn apply_codex_aggregation(db: &Database, active_id: &str) -> Result<(), AppError> {
    let all = db.get_all_providers("codex")?;
    let active = all
        .get(active_id)
        .ok_or_else(|| AppError::Config(format!("Codex 活跃供应商不存在: {active_id}")))?;

    let merged = merge_codex_model_catalog(active, &all);
    let mut updated = active.clone();
    if let Some(obj) = updated.settings_config.as_object_mut() {
        obj.insert("modelCatalog".to_string(), merged);
    }
    db.save_provider("codex", &updated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(id: &str, models: &[&str], aggregate: bool) -> Provider {
        let mut meta = crate::provider::ProviderMeta::default();
        meta.aggregate_enabled = Some(aggregate);
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            settings_config: json!({
                "modelCatalog": {
                    "models": models.iter().map(|m| json!({ "model": m })).collect::<Vec<_>>()
                }
            }),
            website_url: None,
            category: Some("codex".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: Some(meta),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn map(providers: Vec<Provider>) -> IndexMap<String, Provider> {
        providers.into_iter().map(|p| (p.id.clone(), p)).collect()
    }

    #[test]
    fn resolve_routes_to_first_aggregate_provider_with_model() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"], true);
        let gptrelay = provider("gptrelay", &["gpt-4o", "gpt-5"], true);
        let all = map(vec![taotoken, gptrelay]);

        assert_eq!(
            resolve_codex_model_provider("gpt-4o", "taotoken", &all).map(|p| p.id.as_str()),
            Some("gptrelay")
        );
        assert_eq!(
            resolve_codex_model_provider("deepseek-v4-flash", "taotoken", &all)
                .map(|p| p.id.as_str()),
            Some("taotoken")
        );
        assert!(resolve_codex_model_provider("unknown", "taotoken", &all).is_none());
    }

    #[test]
    fn merge_dedups_and_tags_provider_id() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"], true);
        let gptrelay = provider("gptrelay", &["gpt-4o"], true);
        let all = map(vec![taotoken, gptrelay]);

        let merged = merge_codex_model_catalog(&all["taotoken"], &all);
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 3);
        let ids: Vec<String> = models
            .iter()
            .map(|e| e["model"].as_str().unwrap().to_string())
            .collect();
        assert!(ids.contains(&"deepseek-v4-flash".to_string()));
        assert!(ids.contains(&"glm-5".to_string()));
        assert!(ids.contains(&"gpt-4o".to_string()));
        let gpt = models.iter().find(|e| e["model"] == "gpt-4o").unwrap();
        assert_eq!(gpt["providerId"], "gptrelay");
    }

    #[test]
    fn binding_overrides_duplicate_model() {
        // 两家都有 gpt-4o，绑定到 gptrelay
        let taotoken = provider("taotoken", &["gpt-4o"], true);
        let gptrelay = provider("gptrelay", &["gpt-4o"], true);
        let mut all = map(vec![taotoken, gptrelay]);
        all.get_mut("taotoken")
            .unwrap()
            .meta
            .as_mut()
            .unwrap()
            .aggregate_model_bindings = [("gpt-4o".to_string(), "gptrelay".to_string())]
            .into_iter()
            .collect();

        let merged = merge_codex_model_catalog(&all["taotoken"], &all);
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["providerId"], "gptrelay");
        assert_eq!(
            resolve_codex_model_provider("gpt-4o", "taotoken", &all).map(|p| p.id.as_str()),
            Some("gptrelay")
        );
    }
}
