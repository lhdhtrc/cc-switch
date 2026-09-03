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
//! {
//!   "enabled": true,
//!   "providers": ["taotoken","gptrelay"],
//!   "weights": { "taotoken": 100, "gptrelay": 200 },
//!   "bindings": { "gpt-4o": "gptrelay" }
//! }
//! ```

use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// Pseudo provider used as the live-config skeleton in aggregation mode.
///
/// Aggregation is a cc-switch owned mode, so it must not inherit a specific
/// relay's stored config (that is what made provider-specific `[features]`
/// and endpoint fields leak into the merged live config). Routing still picks
/// real relay providers per model; this provider only builds the config.toml
/// and merged model catalog shown to Codex.
pub const CODEX_AGGREGATION_PROVIDER_ID: &str = "codex-aggregation";
pub const CODEX_AGGREGATION_PROVIDER_NAME: &str = "Codex 聚合";
pub const DEFAULT_CODEX_AGGREGATION_WEIGHT: u32 = 100;

const CODEX_AGGREGATION_CONFIG_TEMPLATE: &str = r#"model_provider = "custom"
model_reasoning_effort = "high"
disable_response_storage = true

[model_providers.custom]
name = "Codex 聚合"
wire_api = "responses"
requires_openai_auth = false

[features.multi_agent_v2]
enabled = true
expose_spawn_agent_model_overrides = true
hide_spawn_agent_metadata = false
"#;

/// Codex 聚合配置（存 DB settings 表）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexAggregationConfig {
    /// 聚合模式是否开启
    #[serde(default)]
    pub enabled: bool,
    /// 参与聚合的供应商 id 集合
    #[serde(default)]
    pub providers: HashSet<String>,
    /// 参与聚合供应商的权重：provider id -> weight。未配置时按默认权重处理。
    #[serde(default)]
    pub weights: HashMap<String, u32>,
    /// 同名模型的来源绑定：model id -> provider id
    #[serde(default)]
    pub bindings: HashMap<String, String>,
    /// 聚合模式默认模型（写入 live config 的 `model =` 行；None 时用合并目录首个可见模型）。
    /// 单值、互斥：设置新默认模型会替换旧值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
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

    /// 返回参与聚合供应商的权重；未显式设置时使用默认权重。
    pub fn provider_weight(&self, id: &str) -> u32 {
        self.weights
            .get(id)
            .copied()
            .unwrap_or(DEFAULT_CODEX_AGGREGATION_WEIGHT)
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

fn first_visible_model(catalog: &Value) -> Option<String> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| {
            models.iter().find(|entry| {
                !entry
                    .get("hidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
        })
        .and_then(|entry| entry.get("model"))
        .and_then(Value::as_str)
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

/// 按配置权重降序返回参与聚合的供应商；权重相同时保持数据库原有顺序。
pub fn sorted_enabled_codex_providers<'a>(
    all: &'a IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Vec<&'a Provider> {
    let mut providers: Vec<&Provider> = all
        .values()
        .filter(|provider| config.providers.contains(&provider.id))
        .collect();
    providers.sort_by(|a, b| {
        let aw = config.provider_weight(&a.id);
        let bw = config.provider_weight(&b.id);
        bw.cmp(&aw)
    });
    providers
}

/// 解析某个模型可路由的同名候选链。
///
/// - `bindings[model]` 存在时只返回绑定供应商（同名模型手动锁源，不参与自动切换）；
/// - 否则返回所有提供该模型的启用供应商，按权重降序排列，
///   第一项是当前请求主用中转，后续项是失败时按权重切换的故障转移链。
pub fn resolve_codex_model_provider_chain<'a>(
    model: &str,
    all: &'a IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Vec<&'a Provider> {
    if let Some(bound) = config.bindings.get(model) {
        if config.providers.contains(bound) {
            if let Some(provider) = all.get(bound) {
                return vec![provider];
            }
        }
        return Vec::new();
    }

    let mut providers: Vec<&Provider> = all
        .values()
        .filter(|provider| {
            config.providers.contains(&provider.id) && provider_has_model(provider, model)
        })
        .collect();
    providers.sort_by(|a, b| {
        let aw = config.provider_weight(&a.id);
        let bw = config.provider_weight(&b.id);
        bw.cmp(&aw)
    });
    providers
}

/// 解析某个模型应路由的主用中转（候选链第一项）。
pub fn resolve_codex_model_provider<'a>(
    model: &str,
    all: &'a IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Option<&'a Provider> {
    resolve_codex_model_provider_chain(model, all, config)
        .into_iter()
        .next()
}

/// 解析聚合模式下的路由基准供应商 id（默认模型来源 + 合并目录基准）。
///
/// 取启用集合中权重最高的供应商；权重相同时保持数据库原有顺序。
/// 它只参与模型路由/目录合并，不再作为 live 配置骨架。
/// 聚合未开启或集合为空时返回 `None`。
pub fn resolve_codex_base_provider_id(
    db: &Database,
    config: &CodexAggregationConfig,
) -> Option<String> {
    if !config.enabled || config.providers.is_empty() {
        return None;
    }
    let all = db.get_all_providers("codex").ok()?;
    sorted_enabled_codex_providers(&all, config)
        .first()
        .map(|provider| provider.id.clone())
}

/// 合并参与聚合的中转的模型目录。
///
/// - 仅合并 `config.providers` 中的供应商，权重高者优先（同名去重时高权重条目胜出）；
/// - 按 `model` id 去重（首个出现胜出）；
/// - `bindings` 指定 model -> providerId 时只允许该中转的条目胜出；
/// - 每条目写入 `providerId` 标注来源中转。
pub fn merge_codex_model_catalog(
    all: &IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for provider in sorted_enabled_codex_providers(all, config) {
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
/// - 聚合开启且有启用供应商时：返回「聚合合成骨架 + 合并模型目录」，
///   由调用方走代理接管同步（base_url 会改写为本地代理）；
/// - 聚合关闭或无启用供应商时：返回活跃供应商本身（单供应商模式）。
///
/// 聚合模式下的 live 骨架是合成供应商
/// [`CODEX_AGGREGATION_PROVIDER_ID`]，不再借用任何一家中转的存储配置。
pub fn build_live_provider(db: &Database, active_id: &str) -> Result<Provider, AppError> {
    let config = CodexAggregationConfig::load(db);
    if config.enabled && !config.providers.is_empty() {
        let all = db.get_all_providers("codex")?;
        return build_aggregation_live_provider(&all, &config);
    }

    let all = db.get_all_providers("codex")?;
    all.get(active_id)
        .cloned()
        .ok_or_else(|| AppError::Config(format!("Codex 活跃供应商不存在: {active_id}")))
}

fn build_aggregation_live_provider(
    all: &IndexMap<String, Provider>,
    config: &CodexAggregationConfig,
) -> Result<Provider, AppError> {
    let merged = merge_codex_model_catalog(all, config);
    let mut config_text = CODEX_AGGREGATION_CONFIG_TEMPLATE.to_string();
    let model = config
        .default_model
        .clone()
        .or_else(|| first_visible_model(&merged))
        .ok_or_else(|| AppError::Config("聚合目录中没有可见模型".to_string()))?;
    config_text = crate::codex_config::update_codex_toml_field(&config_text, "model", &model)
        .map_err(AppError::Message)?;

    let settings = json!({
        "auth": {},
        "config": config_text,
        "modelCatalog": merged,
    });
    Ok(Provider::with_id(
        CODEX_AGGREGATION_PROVIDER_ID.to_string(),
        CODEX_AGGREGATION_PROVIDER_NAME.to_string(),
        settings,
        None,
    ))
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
            weights: HashMap::new(),
            bindings: HashMap::new(),
            default_model: None,
        }
    }

    #[test]
    fn resolve_routes_to_enabled_provider_with_model() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        let gptrelay = provider("gptrelay", &["gpt-4o", "gpt-5"]);
        let all = map(vec![taotoken, gptrelay]);
        let config = cfg(true, &["taotoken", "gptrelay"]);

        assert_eq!(
            resolve_codex_model_provider("gpt-4o", &all, &config).map(|p| p.id.as_str()),
            Some("gptrelay")
        );
        assert_eq!(
            resolve_codex_model_provider("deepseek-v4-flash", &all, &config).map(|p| p.id.as_str()),
            Some("taotoken")
        );
        assert!(resolve_codex_model_provider("unknown", &all, &config).is_none());
    }

    #[test]
    fn merge_dedups_and_tags_provider_id() {
        let taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        let gptrelay = provider("gptrelay", &["gpt-4o"]);
        let all = map(vec![taotoken, gptrelay]);
        let config = cfg(true, &["taotoken", "gptrelay"]);

        let merged = merge_codex_model_catalog(&all, &config);
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

        let merged = merge_codex_model_catalog(&all, &config);
        let models = merged["models"].as_array().unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["providerId"], "gptrelay");
        assert_eq!(
            resolve_codex_model_provider("gpt-4o", &all, &config).map(|p| p.id.as_str()),
            Some("gptrelay")
        );
    }

    #[test]
    fn weighted_chain_and_merge_prefer_highest_weight_provider() {
        let taotoken = provider("taotoken", &["gpt-4o"]);
        let devpoolai = provider("devpoolai", &["gpt-4o"]);
        let gptrelay = provider("gptrelay", &["gpt-4o"]);
        let all = map(vec![taotoken, devpoolai, gptrelay]);
        let mut config = cfg(true, &["taotoken", "devpoolai", "gptrelay"]);
        config.weights.insert("taotoken".to_string(), 50);
        config.weights.insert("devpoolai".to_string(), 200);
        config.weights.insert("gptrelay".to_string(), 100);

        let chain = resolve_codex_model_provider_chain("gpt-4o", &all, &config);
        let ids: Vec<&str> = chain.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["devpoolai", "gptrelay", "taotoken"]);

        let merged = merge_codex_model_catalog(&all, &config);
        assert_eq!(merged["models"][0]["providerId"], "devpoolai");
    }

    #[test]
    fn binding_stays_single_route_even_with_weights() {
        let taotoken = provider("taotoken", &["gpt-4o"]);
        let devpoolai = provider("devpoolai", &["gpt-4o"]);
        let all = map(vec![taotoken, devpoolai]);
        let mut config = cfg(true, &["taotoken", "devpoolai"]);
        config.weights.insert("devpoolai".to_string(), 999);
        config
            .bindings
            .insert("gpt-4o".to_string(), "taotoken".to_string());

        let chain = resolve_codex_model_provider_chain("gpt-4o", &all, &config);
        assert_eq!(
            chain.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["taotoken"]
        );
    }

    #[test]
    fn resolve_base_returns_highest_weight_provider() {
        let db = crate::database::Database::memory().expect("memory db");
        let mut taotoken = provider("taotoken", &["deepseek-v4-flash"]);
        taotoken.sort_index = Some(1);
        let mut devpoolai = provider("devpoolai", &["gpt-5.5"]);
        devpoolai.sort_index = Some(2);
        db.save_provider("codex", &taotoken).expect("save taotoken");
        db.save_provider("codex", &devpoolai)
            .expect("save devpoolai");

        let mut config = CodexAggregationConfig {
            enabled: true,
            providers: ["taotoken".into(), "devpoolai".into()]
                .into_iter()
                .collect(),
            weights: HashMap::new(),
            bindings: HashMap::new(),
            default_model: None,
        };
        config.weights.insert("devpoolai".to_string(), 300);
        assert_eq!(
            resolve_codex_base_provider_id(&db, &config).as_deref(),
            Some("devpoolai")
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
    fn build_live_provider_uses_aggregation_skeleton_in_aggregation_mode() {
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
            weights: HashMap::new(),
            bindings: HashMap::new(),
            default_model: Some("gpt-5.5".to_string()),
        };
        config.save(&db).expect("save aggregation config");

        // 即使传入的单供应商 active_id 是 devpoolai，聚合模式也应以
        // 聚合专用合成骨架作为 live 配置，不再借用 devpoolai/taotoken 的存储配置。
        let live = build_live_provider(&db, "devpoolai").expect("build live provider");
        assert_eq!(live.id, CODEX_AGGREGATION_PROVIDER_ID);
        assert_eq!(live.name, CODEX_AGGREGATION_PROVIDER_NAME);
        let config_text = live
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        assert!(
            config_text.contains("[features.multi_agent_v2]")
                && config_text.contains("expose_spawn_agent_model_overrides = true"),
            "aggregation skeleton must carry multi-agent v2 config: {config_text}"
        );
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

    #[test]
    fn build_live_provider_overrides_default_model() {
        let db = crate::database::Database::memory().expect("memory db");
        let mut taotoken = provider("taotoken", &["deepseek-v4-flash", "glm-5"]);
        taotoken.sort_index = Some(1);
        taotoken.settings_config["config"] =
            json!("model = \"deepseek-v4-flash\"\nmodel_reasoning_effort = \"low\"\n");
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
            weights: HashMap::new(),
            bindings: HashMap::new(),
            default_model: Some("gpt-5.5".to_string()),
        };
        config.save(&db).expect("save aggregation config");

        let live = build_live_provider(&db, "taotoken").expect("build live provider");
        assert_eq!(live.id, CODEX_AGGREGATION_PROVIDER_ID);
        let config_text = live
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .expect("config text");
        assert!(
            config_text.contains("model = \"gpt-5.5\""),
            "default model must be overridden: {config_text}"
        );
        assert!(
            !config_text.contains("deepseek-v4-flash"),
            "skeleton default must be replaced: {config_text}"
        );
    }
}
