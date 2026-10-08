//! Discover missing relay catalogs without replacing user model metadata.

use serde_json::{json, Value};
use std::collections::HashSet;

use crate::{app_config::AppType, provider::Provider, store::AppState};

fn has_catalog(provider: &Provider) -> bool {
    provider.settings_config["modelCatalog"]["models"]
        .as_array()
        .is_some_and(|models| {
            models.iter().any(|entry| {
                entry["model"]
                    .as_str()
                    .is_some_and(|id| !id.trim().is_empty())
            })
        })
}

fn merge_ids(provider: &mut Provider, ids: &[String]) {
    let mut entries = provider.settings_config["modelCatalog"]["models"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut seen: HashSet<String> = entries
        .iter()
        .filter_map(|entry| entry["model"].as_str().map(str::to_string))
        .collect();
    for id in ids {
        let id = id.trim();
        if !id.is_empty() && seen.insert(id.to_string()) {
            entries.push(json!({"model": id, "displayName": id}));
        }
    }
    if !provider.settings_config["modelCatalog"].is_object() {
        provider.settings_config["modelCatalog"] = json!({});
    }
    provider.settings_config["modelCatalog"]["models"] = json!(entries);
}

/// Existing catalogs (including deliberately hidden models) are never refetched
/// automatically. Official OAuth providers use their own model discovery.
pub async fn ensure_catalog(state: &AppState, id: &str) -> Result<(), String> {
    let provider = state
        .db
        .get_provider_by_id(id, "codex")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Codex 供应商不存在".to_string())?;
    if has_catalog(&provider) || crate::proxy::providers::is_codex_official_provider(&provider) {
        return Ok(());
    }
    fetch_catalog(state, id).await.map(|_| ())
}

pub async fn fetch_catalog(state: &AppState, id: &str) -> Result<Vec<String>, String> {
    let original = state
        .db
        .get_provider_by_id(id, "codex")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Codex 供应商不存在".to_string())?;
    let text = original
        .settings_config
        .get("config")
        .and_then(Value::as_str);
    let base_url = text
        .and_then(crate::codex_config::extract_codex_base_url)
        .ok_or_else(|| "无法从供应商配置解析 base_url".to_string())?;
    let key =
        crate::codex_config::extract_codex_api_key(original.settings_config.get("auth"), text)
            .unwrap_or_default();
    let fetched =
        super::model_fetch::fetch_models(&base_url, &key, false, None, None, None, None).await?;
    let ids: Vec<String> = fetched.into_iter().map(|model| model.id).collect();
    if ids.iter().all(|id| id.trim().is_empty()) {
        return Err("供应商 /models 返回空模型列表，请在供应商编辑中配置模型目录".to_string());
    }

    // Serialize with provider edits, and reload after the network request so
    // fetching cannot overwrite new credentials, hidden flags or deletions.
    let _guard = crate::mode::controller::lock_settled(state, &AppType::Codex)
        .await
        .map_err(|e| e.to_string())?;
    let mut current = state
        .db
        .get_provider_by_id(id, "codex")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Codex 供应商已删除".to_string())?;
    if current.settings_config["config"] != original.settings_config["config"]
        || current.settings_config["auth"] != original.settings_config["auth"]
    {
        return Err("供应商配置已变化，请重新拉取模型".to_string());
    }
    merge_ids(&mut current, &ids);
    state
        .db
        .save_provider("codex", &current)
        .map_err(|e| e.to_string())?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_audit_fetched_models_preserve_metadata_and_deduplicate() {
        let mut provider = Provider::with_id(
            "relay".into(),
            "relay".into(),
            json!({
                "auth": {}, "config": "model = \"old\"",
                "modelCatalog": {
                    "source": "manual",
                    "models": [{"model": "old", "hidden": true, "apiFormat": "chat"}]
                }
            }),
            None,
        );
        merge_ids(
            &mut provider,
            &["old".into(), "new".into(), "new".into(), " ".into()],
        );
        assert_eq!(
            provider.settings_config["modelCatalog"]["models"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            provider.settings_config["modelCatalog"]["models"][0]["hidden"],
            true
        );
        assert_eq!(
            provider.settings_config["modelCatalog"]["models"][0]["apiFormat"],
            "chat"
        );
        assert_eq!(provider.settings_config["modelCatalog"]["source"], "manual");
        assert!(has_catalog(&provider));
    }
}
