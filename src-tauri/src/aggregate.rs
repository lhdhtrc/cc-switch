//! 多中转聚合模块（Codex）
//!
//! 聚合是独立于单供应商模式的一等功能（两种模式）：
//! - 单供应商模式：沿用原有行为，live 配置写当前活跃供应商；
//! - 聚合模式：在独立聚合页启用多个中转，模型目录按启用集合合并写入
//!   Codex live 配置（不修改各供应商的存储目录），代理按请求模型路由到
//!   提供该模型的中转（同名模型可通过 binding 指定来源）。
//!
//! 聚合配置保存在数据库 settings 表（key = `codex_aggregation`）：
//! ```json
//! { "enabled": true, "providers": ["taotoken","gptrelay"], "bindings": { "gpt-4o": "gptrelay" } }
//! ```

use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Codex 聚合配置（存 DB settings 表）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexAggregationConfig {
    /// 聚合模式是否开启
    #[serde(default)]
    pub enabled: bool,
    /// 参与聚合的供应商 id 集合
    #[serde(default)]
    pub providers: HashSet<String>,
    /// 同名模型的来源绑定：model id -> provider id
    #[serde(default)]
    pub bindings: HashMap<String, String>,
}

impl CodexAggregationConfig {
    const KEY: &'static str = "codex_aggregation";

    /// 从数据库 settings 表读取聚合配置。
    pub fn load(db: &Database) -> Self {
        db.get_setting(Self::KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// 保存聚合配置到数据库 settings 表。
    pub fn save(&self, db: &Database) -> Result<(), AppError> {
        let raw = serde_json::to_string(self)
            .map_err(|e| AppError::Config(format!("序列化聚合配置失败: {e}")))?;
        db.set_setting(Self::KEY, &raw)?;
        Ok(())
    }
}

/// 读取模型目录条目里的来源中转 id（聚合写入）。
fn entry_provider_id(entry: &Value) -> Option<String> {
    entry
        .get("providerId")
        .or_else(|| entry.get("provider_id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 供应商目录中是否存在某个模型 id。
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

/// 解析某个模型应路由到哪个中转。
///
/// 优先级：
/// 1. `bindings[model]` 显式绑定；
/// 2. 骨架供应商（启用集合第一个，用于合并目录/默认模型的内部基准）；
/// 3. 其余参与聚合的供应商（按存储顺序）第一个提供该模型者。
pub fn resolve_codex_model_provider<'a>(
    model: &str,
    active_id: &str,
    all: &'a IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Option<&'a Provider> {
    if let Some(bound) = config.bindings.get(model) {
        if let Some(provider) = all.get(bound) {
            return Some(provider);
        }
    }

    if let Some(active) = all.get(active_id) {
        if config.providers.contains(active_id) && provider_has_model(active, model) {
            return Some(active);
        }
    }

    all.values().find(|provider| {
        provider.id != active_id
            && config.providers.contains(&provider.id)
            && provider_has_model(provider, model)
    })
}

/// 解析聚合模式下的骨架供应商 id（live 配置骨架 + 默认模型来源 + 合并目录基准）。
///
/// 取启用集合中第一个供应商（按存储顺序）；启用供应商之间没有用户可见的主次之分。
/// 聚合未开启或集合为空时返回 `None`。
pub fn resolve_codex_base_provider_id(
    db: &Database,
    config: &CodexAggregationConfig,
) -> Option<String> {
    if !config.enabled || config.providers.is_empty() {
        return None;
    }
    let all = db.get_all_providers("codex").ok()?;
    all.values()
        .find(|provider| config.providers.contains(&provider.id))
        .map(|provider| provider.id.clone())
}

/// 合并参与聚合的中转的模型目录。
///
/// - 仅合并 `config.providers` 中的供应商；活跃供应商优先；
/// - 按 `model` id 去重（首个出现胜出）；
/// - `bindings` 指定 model -> providerId 时只允许该中转的条目胜出；
/// - 每条目写入 `providerId` 标注来源中转。
pub fn merge_codex_model_catalog(
    active: &Provider,
    all: &IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let mut order: Vec<&Provider> = Vec::new();
    if config.providers.contains(&active.id) {
        order.push(active);
    }
    for provider in all.values() {
        if provider.id != active.id && config.providers.contains(&provider.id) {
            order.push(provider);
        }
    }

    for provider in order {
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

            let owner = entry_provider_id(&entry).unwrap_or_else(|| provider.id.clone());
            if owner != provider.id {
                continue;
            }

            if let Some(bound) = config.bindings.get(&model_id) {
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

/// 构建用于写入 Codex live 配置的 provider：
/// - 聚合开启且有启用供应商时：返回「骨架供应商 + 合并模型目录」的克隆，
///   由调用方走代理接管同步（base_url 会改写为本地代理）；
/// - 聚合关闭或无启用供应商时：返回活跃供应商本身（单供应商模式）。
///
/// 聚合模式下的骨架供应商见 [`resolve_codex_base_provider_id`]，
/// 与单供应商模式的当前供应商解耦。
pub fn build_live_provider(db: &Database, active_id: &str) -> Result<Provider, AppError> {
    let all = db.get_all_providers("codex")?;
    let active = all
        .get(active_id)
        .cloned()
        .ok_or_else(|| AppError::Config(format!("Codex 活跃供应商不存在: {active_id}")))?;

    let config = CodexAggregationConfig::load(db);
    if !config.enabled || config.providers.is_empty() {
        return Ok(active);
    }

    let base_id = resolve_codex_base_provider_id(db, &config)
        .ok_or_else(|| AppError::Config("聚合模式缺少骨架供应商".to_string()))?;
    let base = all
        .get(&base_id)
        .cloned()
        .ok_or_else(|| AppError::Config(format!("聚合骨架供应商不存在: {base_id}")))?;

    let merged = merge_codex_model_catalog(&base, &all, &config);
    let mut merged_provider = base.clone();
    if let Some(obj) = merged_provider.settings_config.as_object_mut() {
        obj.insert("modelCatalog".to_string(), merged);
    }
    Ok(merged_provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, models: &[&str]) -> Provider {
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
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn map(providers: Vec<Provider>) -> IndexMap<String, Provider> {
        providers.into_iter().map(|p| (p.id.clone(), p)).collect()
    }

    fn cfg(enabled: bool, ids: &[&str]) -> CodexAggregationConfig {
        CodexAggregationConfig {
            enabled,
            providers: ids.iter().map(|s| s.to_string()).collect(),
            bindings: HashMap::new(),
        }
    }

    #[test]
    fn resolve_routes_to_enabled_provider_with_model() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        let gptrelay = provider("gptrelay", &["gpt-4o", "gpt-5"]);
        let all = map(vec![taotoken, gptrelay]);
        let config = cfg(true, &["taotoken", "gptrelay"]);

        assert_eq!(
            resolve_codex_model_provider("gpt-4o", "taotoken", &all, &config)
                .map(|p| p.id.as_str()),
            Some("gptrelay")
        );
        assert_eq!(
            resolve_codex_model_provider("deepseek-v4-flash", "taotoken", &all, &config)
                .map(|p| p.id.as_str()),
            Some("taotoken")
        );
        assert!(resolve_codex_model_provider("unknown", "taotoken", &all, &config).is_none());
    }

    #[test]
    fn merge_dedups_and_tags_provider_id() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        let gptrelay = provider("gptrelay", &["gpt-4o"]);
        let all = map(vec![taotoken, gptrelay]);
        let config = cfg(true, &["taotoken", "gptrelay"]);

        let merged = merge_codex_model_catalog(&all["taotoken"], &all, &config);
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 3);
        let gpt = models.iter().find(|e| e["model"] == "gpt-4o").unwrap();
        assert_eq!(gpt["providerId"], "gptrelay");
    }

    #[test]
    fn binding_overrides_duplicate_model() {
        let taotoken = provider("taotoken", &["gpt-4o"]);
        let gptrelay = provider("gptrelay", &["gpt-4o"]);
        let all = map(vec![taotoken, gptrelay]);
        let mut config = cfg(true, &["taotoken", "gptrelay"]);
        config
            .bindings
            .insert("gpt-4o".to_string(), "gptrelay".to_string());

        let merged = merge_codex_model_catalog(&all["taotoken"], &all, &config);
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["providerId"], "gptrelay");
        assert_eq!(
            resolve_codex_model_provider("gpt-4o", "taotoken", &all, &config)
                .map(|p| p.id.as_str()),
            Some("gptrelay")
        );
    }

    #[test]
    fn resolve_base_returns_first_enabled_provider() {
        let db = crate::database::Database::memory().expect("memory db");
        let mut taotoken = provider("taotoken", &["deepseek-v4-flash"]);
        taotoken.sort_index = Some(1);
        let mut devpoolai = provider("devpoolai", &["gpt-5.5"]);
        devpoolai.sort_index = Some(2);
        db.save_provider("codex", &taotoken).expect("save taotoken");
        db.save_provider("codex", &devpoolai)
            .expect("save devpoolai");

        // 骨架供应商 = 启用集合第一个（按存储顺序），无主次之分。
        let config = CodexAggregationConfig {
            enabled: true,
            providers: ["taotoken".into(), "devpoolai".into()]
                .into_iter()
                .collect(),
            bindings: HashMap::new(),
        };
        assert_eq!(
            resolve_codex_base_provider_id(&db, &config).as_deref(),
            Some("taotoken")
        );

        // 聚合未开启 / 集合为空时返回 None。
        let disabled = CodexAggregationConfig {
            enabled: false,
            ..config.clone()
        };
        assert!(resolve_codex_base_provider_id(&db, &disabled).is_none());
        let empty = CodexAggregationConfig {
            enabled: true,
            providers: HashSet::new(),
            ..config
        };
        assert!(resolve_codex_base_provider_id(&db, &empty).is_none());
    }

    #[test]
    fn build_live_provider_uses_base_in_aggregation_mode() {
        let db = crate::database::Database::memory().expect("memory db");
        let mut taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        taotoken.sort_index = Some(1);
        let mut devpoolai = provider("devpoolai", &["gpt-5.5"]);
        devpoolai.sort_index = Some(2);
        db.save_provider("codex", &taotoken).expect("save taotoken");
        db.save_provider("codex", &devpoolai)
            .expect("save devpoolai");

        let config = CodexAggregationConfig {
            enabled: true,
            providers: ["taotoken".into(), "devpoolai".into()]
                .into_iter()
                .collect(),
            bindings: HashMap::new(),
        };
        config.save(&db).expect("save aggregation config");

        // 即使传入的单供应商 active_id 是 devpoolai，聚合模式也应以骨架供应商
        // taotoken（启用集合第一个）作为 live 配置骨架，并注入合并目录。
        let live = build_live_provider(&db, "devpoolai").expect("build live provider");
        assert_eq!(live.id, "taotoken");
        let models = live
            .settings_config
            .get("modelCatalog")
            .and_then(|c| c.get("models"))
            .and_then(|m| m.as_array())
            .expect("merged models");
        let ids: Vec<&str> = models
            .iter()
            .filter_map(|m| m.get("model").and_then(|v| v.as_str()))
            .collect();
        assert!(ids.contains(&"gpt-5.5"));
        assert!(ids.contains(&"deepseek-v4-flash"));
        assert!(ids.contains(&"glm-5"));
    }
}
