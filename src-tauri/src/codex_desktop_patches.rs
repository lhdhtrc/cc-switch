//! Feature-specific CDP patches for Codex Desktop.
//!
//! The script injected through CDP makes the renderer show the reasoning
//! effort selector for every model in the CC Switch model catalog instead of
//! only the OpenAI whitelist:
//! - rewrites the Statsig dynamic-config gate `107580212` so hidden models are
//!   shown and the whitelist matches the catalog;
//! - ensures every model descriptor carries `supported_reasoning_levels` /
//!   `default_reasoning_level` (the data the picker uses to render the
//!   dropdown);
//! - patches model-list responses and already-mounted React state.

use crate::codex_desktop::{codex_cdp_status, CodexCdpStatus, CodexDesktop};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexUnlockResult {
    /// `injected`
    pub state: &'static str,
    pub message: String,
    pub port: Option<u16>,
    pub injected_targets: usize,
    pub model_count: usize,
}

pub async fn check_cdp_status() -> CodexCdpStatus {
    codex_cdp_status().await
}

/// Unlock the thinking strength.  Launches Codex Desktop with remote debugging when
/// it is not running and injects the compatibility script into every Codex
/// renderer page.
pub async fn unlock_reasoning_effort() -> Result<CodexUnlockResult, String> {
    let _ = ensure_desktop_reasoning_allowlist();
    let cdp = CodexDesktop::new().await?;
    let catalog = load_catalog_payload();
    let script = reasoning_effort_unlock_js(&catalog);
    let responses = cdp.install_script(&script).await?;

    let mut injected = 0usize;
    let mut exceptions = Vec::new();
    for response in &responses {
        if let Some(exception) = response
            .get("result")
            .and_then(|r| r.get("exceptionDetails"))
        {
            exceptions.push(format!("JS injection error: {exception}"));
            continue;
        }
        injected += 1;
    }
    if injected == 0 && responses.is_empty() {
        return Err("No Codex Desktop page target could be reached for injection.".to_string());
    }

    let model_count = catalog
        .get("models")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let message = if injected > 0 {
        format!(
            "Thinking strength unlocked via CDP on port {} ({} page{}, {} catalog models)",
            cdp.port,
            injected,
            if injected == 1 { "" } else { "s" },
            model_count
        )
    } else {
        format!(
            "Thinking strength script could not run in Codex Desktop: {}",
            exceptions.join("; ")
        )
    };
    if injected == 0 {
        return Err(message);
    }
    Ok(CodexUnlockResult {
        state: "injected",
        message,
        port: Some(cdp.port),
        injected_targets: injected,
        model_count,
    })
}

// ---------------------------------------------------------------------------
// Desktop [desktop] enabled-reasoning-efforts allowlist
// ---------------------------------------------------------------------------

const CANONICAL_REASONING_EFFORTS: &[&str] = &[
    "none",
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
    "max",
    "ultra",
    "persistent",
];

/// Codex Desktop filters the visible thinking-strength options through the
/// `[desktop] enabled-reasoning-efforts` list in `~/.codex/config.toml`.
/// Merge every known effort into that list so menu choices are not silently
/// dropped after the CDP patch adds per-model capabilities. Idempotent.
fn ensure_desktop_reasoning_allowlist() -> Result<(), String> {
    let path = crate::codex_config::get_codex_config_dir().join("config.toml");
    if !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {} failed: {error}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| format!("parse {} failed: {error}", path.display()))?;

    let mut existing = Vec::new();
    if let Some(table) = doc.get("desktop").and_then(toml_edit::Item::as_table) {
        if let Some(array) = table
            .get("enabled-reasoning-efforts")
            .and_then(toml_edit::Item::as_array)
        {
            for value in array.iter() {
                if let Some(effort) = value.as_str() {
                    existing.push(effort.to_string());
                }
            }
        }
    }

    let mut merged = existing.clone();
    for effort in CANONICAL_REASONING_EFFORTS {
        if !merged.iter().any(|candidate| candidate == effort) {
            merged.push((*effort).to_string());
        }
    }
    if merged == existing {
        return Ok(());
    }

    if !doc.as_table().contains_key("desktop") {
        doc["desktop"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let mut array = toml_edit::Array::new();
    for effort in &merged {
        array.push(effort);
    }
    doc["desktop"]["enabled-reasoning-efforts"] =
        toml_edit::Item::Value(toml_edit::Value::Array(array));

    std::fs::write(&path, doc.to_string())
        .map_err(|error| format!("write {} failed: {error}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Catalog payload
// ---------------------------------------------------------------------------

/// Read the active model catalog CC Switch writes for Codex so the renderer
/// patch knows exactly which models and reasoning levels to expose.
fn load_catalog_payload() -> Value {
    let catalog_path = crate::codex_config::get_codex_model_catalog_path();
    let models = read_models_from_catalog(&catalog_path)
        .or_else(|| {
            // Fallback: models_cache.json written by CC Switch.
            let cache = crate::codex_config::get_codex_config_dir().join("models_cache.json");
            read_models_from_catalog(&cache)
        })
        .unwrap_or_default();

    let mut payload = json!({ "defaultModel": null, "models": [] });
    payload["models"] = Value::Array(models);
    payload
}

fn read_models_from_catalog(path: &Path) -> Option<Vec<Value>> {
    let catalog = crate::config::read_json_file::<Value>(path).ok()?;
    let entries = catalog.get("models").and_then(Value::as_array)?;
    let mut models = Vec::new();
    for entry in entries {
        let Some(model_name) = entry
            .get("model")
            .or_else(|| entry.get("slug"))
            .or_else(|| entry.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let reasoning_levels = entry
            .get("supported_reasoning_levels")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let default_reasoning_level = entry
            .get("default_reasoning_level")
            .cloned()
            .unwrap_or_else(|| Value::String("medium".to_string()));
        models.push(json!({
            "model": model_name,
            "reasoningLevels": reasoning_levels,
            "defaultReasoningLevel": default_reasoning_level,
        }));
    }
    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

fn reasoning_effort_unlock_js(catalog: &Value) -> String {
    let payload = catalog.to_string();
    SCRIPT_TEMPLATE.replace("/*__PAYLOAD__*/", &payload)
}

// ---------------------------------------------------------------------------
// Injected JavaScript
// ---------------------------------------------------------------------------

const SCRIPT_TEMPLATE: &str = r#"(function () {
  'use strict';
  var CC_KEY = '__ccSwitchCodexCompatibilityV3';
  if (window[CC_KEY]) { return; }
  window[CC_KEY] = { version: 3 };

  var CATALOG = /*__PAYLOAD__*/;
  if (!CATALOG || typeof CATALOG !== 'object') { CATALOG = { defaultModel: null, models: [] }; }
  if (!Array.isArray(CATALOG.models)) { CATALOG.models = []; }
  var MODEL_NAMES = CATALOG.models.map(function (m) { return m && m.model; }).filter(Boolean);

  var FALLBACK_LEVELS = [
    { effort: 'low', description: 'Light reasoning' },
    { effort: 'medium', description: 'Balanced reasoning' },
    { effort: 'high', description: 'Deep reasoning' },
    { effort: 'xhigh', description: 'Extra deep reasoning' },
    { effort: 'max', description: 'Maximum reasoning' },
    { effort: 'persistent', description: 'Persistent reasoning' }
  ];

  function toCamelLevels(levels) {
    var out = [];
    (Array.isArray(levels) ? levels : []).forEach(function (level) {
      if (!isRecord(level)) return;
      var effort = level.reasoningEffort || level.effort;
      if (!effort) return;
      out.push({ reasoningEffort: effort, description: level.description || (effort + ' effort') });
    });
    return out;
  }

  var DESIRED_EFFORTS = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'ultra', 'persistent'];
  function mergeEfforts(existing) {
    if (!Array.isArray(existing)) return existing;
    var seen = {};
    existing.forEach(function (value) { if (value) seen[String(value)] = true; });
    DESIRED_EFFORTS.forEach(function (value) { if (!seen[value]) { seen[value] = true; existing.push(value); } });
    return existing;
  }

  function isRecord(v) { return !!v && typeof v === 'object' && !Array.isArray(v); }
  function slugOf(m) { return (m && (m.model || m.slug || m.id || m.name)) || null; }

  function levelsFor(slug) {
    for (var i = 0; i < CATALOG.models.length; i++) {
      if (CATALOG.models[i].model === slug && Array.isArray(CATALOG.models[i].reasoningLevels) && CATALOG.models[i].reasoningLevels.length > 0) {
        return CATALOG.models[i].reasoningLevels;
      }
    }
    return FALLBACK_LEVELS;
  }
  function defaultLevelFor(slug) {
    for (var i = 0; i < CATALOG.models.length; i++) {
      if (CATALOG.models[i].model === slug && CATALOG.models[i].defaultReasoningLevel) {
        return CATALOG.models[i].defaultReasoningLevel;
      }
    }
    return 'medium';
  }

  function setReasoning(m) {
    if (!isRecord(m)) return false;
    var slug = slugOf(m);
    if (typeof slug !== 'string' || !slug) return false;
    var levels = levelsFor(slug);
    var camelLevels = toCamelLevels(levels);
    var def = m.default_reasoning_level || m.defaultReasoningLevel || defaultLevelFor(slug);
    var changed = false;
    if (!Array.isArray(m.supported_reasoning_levels) || m.supported_reasoning_levels.length === 0) {
      m.supported_reasoning_levels = levels; changed = true;
    }
    if (!Array.isArray(m.supportedReasoningLevels) || m.supportedReasoningLevels.length === 0) {
      m.supportedReasoningLevels = camelLevels; changed = true;
    }
    if (!Array.isArray(m.supportedReasoningEfforts) || m.supportedReasoningEfforts.length === 0) {
      m.supportedReasoningEfforts = camelLevels; changed = true;
    }
    if (m.reasoning_efforts == null) {
      m.reasoning_efforts = levels.map(function (l) { return l.effort; }); changed = true;
    }
    if (m.reasoningEfforts == null) {
      m.reasoningEfforts = levels.map(function (l) { return l.effort; }); changed = true;
    }
    if (!m.default_reasoning_level) { m.default_reasoning_level = def; changed = true; }
    if (!m.defaultReasoningLevel) { m.defaultReasoningLevel = def; changed = true; }
    if (!m.defaultReasoningEffort) { m.defaultReasoningEffort = def; changed = true; }
    return changed;
  }

  function patchEffortAllowlist(obj) {
    if (!isRecord(obj)) return false;
    var changed = false;
    if (Array.isArray(obj.enabledReasoningEfforts)) {
      var before = obj.enabledReasoningEfforts.length;
      mergeEfforts(obj.enabledReasoningEfforts);
      if (obj.enabledReasoningEfforts.length !== before) changed = true;
    }
    if (Array.isArray(obj['enabled-reasoning-efforts'])) {
      var beforeDashed = obj['enabled-reasoning-efforts'].length;
      mergeEfforts(obj['enabled-reasoning-efforts']);
      if (obj['enabled-reasoning-efforts'].length !== beforeDashed) changed = true;
    }
    return changed;
  }

  function ensureModelArray(arr) {
    if (!Array.isArray(arr)) return false;
    var changed = false;
    for (var i = 0; i < arr.length; i++) {
      var item = arr[i];
      if (isRecord(item) && typeof slugOf(item) === 'string') {
        changed = setReasoning(item) || changed;
      }
    }
    return changed;
  }

  function patchGateObject(gate) {
    if (!isRecord(gate) || !isRecord(gate.value)) return false;
    var v = gate.value;
    var changed = false;
    if (v.use_hidden_models === true) { v.use_hidden_models = false; changed = true; }
    if (v.useHiddenModels === true) { v.useHiddenModels = false; changed = true; }
    if (Array.isArray(v.available_models) && v.available_models.length === 0 && MODEL_NAMES.length > 0) {
      v.available_models = MODEL_NAMES.slice(); changed = true;
    }
    if (Array.isArray(v.availableModels) && v.availableModels.length === 0 && MODEL_NAMES.length > 0) {
      v.availableModels = MODEL_NAMES.slice(); changed = true;
    }
    if ((!v.default_model || !v.defaultModel) && CATALOG.defaultModel) {
      if (!v.default_model) { v.default_model = CATALOG.defaultModel; changed = true; }
      if (!v.defaultModel) { v.defaultModel = CATALOG.defaultModel; changed = true; }
    }
    return changed;
  }

  function patchStatsigStore(store) {
    if (!isRecord(store)) return false;
    var config = null;
    if (store.dynamic_configs) {
      config = store.dynamic_configs['107580212'] || store.dynamic_configs[107580212];
    }
    if (!config) config = store['107580212'] || store[107580212];
    if (!config && store.value && (store.value.dynamic_configs)) {
      config = store.value.dynamic_configs['107580212'] || store.value.dynamic_configs[107580212];
    }
    if (!config) return false;
    if (config.value && typeof config.value === 'object') {
      patchGateObject(config);
    }
    if (config.data && typeof config.data === 'object') {
      var wrapped = { value: config.data };
      patchGateObject(wrapped);
    }
    return true;
  }

  function patchModelContainer(obj) {
    if (!isRecord(obj)) return false;
    if (obj === window || obj === document || obj.nodeType === 1 || obj.nodeType === 9) {
      return false;
    }
    var changed = setReasoning(obj);
    changed = patchEffortAllowlist(obj) || changed;
    if (obj.use_hidden_models === true || obj.useHiddenModels === true) {
      if (obj.use_hidden_models !== undefined) { obj.use_hidden_models = false; changed = true; }
      if (obj.useHiddenModels !== undefined) { obj.useHiddenModels = false; changed = true; }
    }
    if (MODEL_NAMES.length > 0) {
      if (Array.isArray(obj.available_models) && obj.available_models.length === 0) { obj.available_models = MODEL_NAMES.slice(); changed = true; }
      if (Array.isArray(obj.availableModels) && obj.availableModels.length === 0) { obj.availableModels = MODEL_NAMES.slice(); changed = true; }
    }
    ['models', 'model_list', 'allModels'].forEach(function (key) {
      if (Array.isArray(obj[key])) changed = ensureModelArray(obj[key]) || changed;
    });
    return changed;
  }

  var seenObjects = null;
  function walkGraph(root, depth) {
    if (!root || depth > 8) return false;
    if (typeof root !== 'object') return false;
    if (seenObjects.has(root)) return false;
    seenObjects.add(root);
    var changed = false;
    try {
      if (root === CATALOG) return false;
      if (root.nodeType === 1 || root.nodeType === 9 || root.nodeType === 11) {
        return false;
      }
      changed = patchModelContainer(root) || changed;
      if (root.enabledReasoningEfforts && Array.isArray(root.enabledReasoningEfforts)) changed = patchEffortAllowlist(root) || changed;
      if (root['enabled-reasoning-efforts']) changed = patchEffortAllowlist(root) || changed;
      if (root.dynamic_configs) changed = patchStatsigStore(root) || changed;
      if (Array.isArray(root)) {
        for (var i = 0; i < root.length; i++) {
          if (root[i] && typeof root[i] === 'object') changed = walkGraph(root[i], depth + 1) || changed;
        }
        return changed;
      }
      var keys = Object.keys(root);
      var budget = Math.min(keys.length, 2000);
      for (var k = 0; k < budget; k++) {
        var child = root[keys[k]];
        if (child && typeof child === 'object' && child !== root) {
          changed = walkGraph(child, depth + 1) || changed;
        }
      }
    } catch (e) {}
    return changed;
  }

  function applyAll() {
    var changed = false;
    try {
      seenObjects = new WeakSet();
      if (window.__STATSIG__) {
        patchStatsigStore(window.__STATSIG__);
        if (window.__STATSIG__.store) patchStatsigStore(window.__STATSIG__.store);
        if (window.__STATSIG__.evaluations) patchStatsigStore(window.__STATSIG__.evaluations);
      }
      if (window.StatsigClient) {
        patchStatsigStore(window.StatsigClient);
        patchStatsigStore(window.StatsigClient.store);
      }
      changed = walkGraph(window.__STATSIG__, 0) || changed;
      changed = walkGraph(window, 2) || changed;
    } catch (e) {}
    return changed;
  }

  function patchResponseBody(body) {
    if (!body || typeof body !== 'object') return false;
    var changed = false;
    if (Array.isArray(body)) return ensureModelArray(body);
    if (isRecord(body)) {
      if (Array.isArray(body.data) && body.data.length) changed = ensureModelArray(body.data) || changed;
      if (Array.isArray(body.models) && body.models.length) changed = ensureModelArray(body.models) || changed;
      if (body.model && typeof body.model === 'string') changed = setReasoning(body) || changed;
      if (body.data && isRecord(body.data) && body.data.models) {
        if (Array.isArray(body.data.models)) changed = ensureModelArray(body.data.models) || changed;
      }
      if (body.data && isRecord(body.data) && body.data.data && Array.isArray(body.data.data)) {
        changed = ensureModelArray(body.data.data) || changed;
      }
    }
    return changed;
  }

  function isModelListRequest(url, bodyText) {
    var text = String(url || '') + ' ' + String(bodyText || '');
    return /model\/list|list-models-for-host|modelCatalog|models/.test(text);
  }

  var origJson = Response.prototype.json;
  Response.prototype.json = function () {
    var self = this;
    return origJson.apply(this, arguments).then(function (data) {
      try {
        var url = (self && self.url) || '';
        if (isModelListRequest(url, '')) {
          patchResponseBody(data);
        } else {
          patchResponseBody(data);
        }
      } catch (e) {}
      return data;
    });
  };

  var origFetch = window.fetch;
  window.fetch = function (input, init) {
    var bodyText = '';
    try {
      bodyText = (init && (init.body || init.bodyUsed && '')) || '';
      if (typeof bodyText !== 'string') bodyText = '';
    } catch (e) { bodyText = ''; }
    return origFetch.apply(this, arguments).then(function (resp) {
      try {
        var url = typeof input === 'string' ? input : (input && input.url) || '';
        if (isModelListRequest(url, bodyText) && resp && typeof resp.clone === 'function') {
          resp.clone().json().then(function (body) {
            try { patchResponseBody(body); } catch (e) {}
          }).catch(function () {});
        }
      } catch (e) {}
      return resp;
    });
  };

  function patchReactFiber(root, depth) {
    if (!root || depth > 16) return;
    try {
      var state = root.memoizedState;
      var guard = 0;
      while (state && guard++ < 40) {
        if (state.memoizedState) patchReactFiber(state.memoizedState, depth + 1);
        var value = state.memoizedState;
        if (value && typeof value === 'object' && value !== root) {
          seenObjects = new WeakSet();
          walkGraph(value, 0);
        }
        state = state.next;
      }
      if (root.child) patchReactFiber(root.child, depth + 1);
      if (root.sibling) patchReactFiber(root.sibling, depth + 1);
    } catch (e) {}
  }

  function patchRoot() {
    try {
      var rootEl = document.getElementById('root') || document.getElementById('app');
      if (!rootEl) return;
      var key = Object.keys(rootEl).find(function (k) {
        return k.indexOf('__reactFiber') === 0 || k.indexOf('__reactInternalInstance') === 0;
      });
      if (key && rootEl[key]) patchReactFiber(rootEl[key], 0);
    } catch (e) {}
  }

  if (typeof MutationObserver !== 'undefined' && document.documentElement) {
    var scheduled = false;
    new MutationObserver(function () {
      if (scheduled) return;
      scheduled = true;
      setTimeout(function () {
        scheduled = false;
        applyAll();
        patchRoot();
        window.dispatchEvent(new CustomEvent('resize'));
      }, 350);
    }).observe(document.documentElement, { childList: true, subtree: true });
  }

  applyAll();
  patchRoot();
  window.setTimeout(function () { applyAll(); patchRoot(); }, 800);
  window.setTimeout(function () { applyAll(); patchRoot(); }, 2500);
  window.dispatchEvent(new CustomEvent('resize'));
  console.log('[cc-switch] Codex Desktop thinking strength layer installed');
})();"#;
