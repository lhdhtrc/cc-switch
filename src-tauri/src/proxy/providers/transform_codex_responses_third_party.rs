//! Generic Codex synthetic-input compatibility for non-OpenAI providers.
//!
//! Codex multi-agent v2 delivers subagent tasks through `agent_message` items
//! whose payload is carried in `encrypted_content`. OpenAI can consume that
//! item natively, but third-party Responses providers such as DeepSeek/Kimi
//! drop or reject it, so the child starts with an empty task.
//!
//! Codex desktop also injects thread delegations and heartbeat wakeups as
//! standalone `function_call_output` items without a `call_id`. Strict
//! third-party Responses providers reject that shape with HTTP 400.
//!
//! Both synthetic carriers carry plaintext instruction text in local proxy
//! setups, so rewriting them to ordinary user messages is safe before the
//! request is forwarded or converted to another wire format.

use serde_json::{json, Value};

/// Rewrite Codex synthetic input items for a non-OpenAI provider request.
///
/// Returns whether any item changed. The caller should log once with provider
/// context.
pub(crate) fn apply_codex_third_party_request_compat(body: &mut Value, provider_id: &str) -> bool {
    let changed = rewrite_codex_agent_message_input_items(body)
        | rewrite_standalone_function_call_outputs(body);
    if changed {
        log::info!(
            "[Codex] Rewrote Codex synthetic input items for third-party provider (provider={provider_id})"
        );
    }
    changed
}

/// Rewrite Codex multi-agent v2 `agent_message` items into ordinary user
/// messages. Walk the whole request body because Codex may nest the item under
/// later collaboration turns.
pub(crate) fn rewrite_codex_agent_message_input_items(body: &mut Value) -> bool {
    rewrite_agent_message_value(body)
}

fn rewrite_agent_message_value(value: &mut Value) -> bool {
    if rewrite_agent_message_item(value) {
        return true;
    }
    match value {
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= rewrite_agent_message_value(item);
            }
            changed
        }
        Value::Object(obj) => {
            let mut changed = false;
            for child in obj.values_mut() {
                changed |= rewrite_agent_message_value(child);
            }
            changed
        }
        _ => false,
    }
}

fn rewrite_agent_message_item(item: &mut Value) -> bool {
    if json_type(item) != Some("agent_message") {
        return false;
    }

    let id = item.get("id").cloned();
    let content = flatten_agent_message_content(item.get("content"));
    if content.is_empty() {
        return false;
    }

    let mut message = json!({
        "type": "message",
        "role": "user",
        "content": content,
    });
    if let Some(id) = id {
        message["id"] = id;
    }
    *item = message;
    true
}

fn flatten_agent_message_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::Array(parts)) => parts.iter().filter_map(part_to_input_text).collect(),
        Some(Value::String(text)) if !text.is_empty() => vec![input_text_part(text)],
        _ => Vec::new(),
    }
}

fn part_to_input_text(part: &Value) -> Option<Value> {
    let text = if json_type(part) == Some("encrypted_content") {
        part.get("encrypted_content")
            .or_else(|| part.get("text"))
            .and_then(Value::as_str)
    } else {
        part.get("text").and_then(Value::as_str)
    }?;
    if text.is_empty() {
        None
    } else {
        Some(input_text_part(text))
    }
}

fn input_text_part(text: &str) -> Value {
    json!({ "type": "input_text", "text": text })
}

/// Rewrite standalone `function_call_output` items that lack a `call_id`.
///
/// Codex desktop injects `<codex_delegation>` and `<heartbeat>` wakeups as
/// synthetic function call outputs with no matching function call. OpenAI
/// tolerates them, but strict Responses providers reject the item because a
/// `function_call_output` requires a `call_id`. The instruction text is not a
/// real tool result, so promote it to a user message.
pub(crate) fn rewrite_standalone_function_call_outputs(body: &mut Value) -> bool {
    rewrite_function_call_output_value(body)
}

fn rewrite_function_call_output_value(value: &mut Value) -> bool {
    if rewrite_standalone_function_call_output_item(value) {
        return true;
    }
    match value {
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= rewrite_function_call_output_value(item);
            }
            changed
        }
        Value::Object(obj) => {
            let mut changed = false;
            for child in obj.values_mut() {
                changed |= rewrite_function_call_output_value(child);
            }
            changed
        }
        _ => false,
    }
}

fn rewrite_standalone_function_call_output_item(item: &mut Value) -> bool {
    if json_type(item) != Some("function_call_output") {
        return false;
    }

    let has_call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .is_some_and(|call_id| !call_id.is_empty());
    if has_call_id {
        return false;
    }

    let Some(output) = tool_output_text(item.get("output")) else {
        return false;
    };
    if !is_synthetic_instruction(&output) {
        return false;
    }

    let id = item.get("id").cloned();
    let mut message = json!({
        "type": "message",
        "role": "user",
        "content": [input_text_part(&output)],
    });
    if let Some(id) = id {
        message["id"] = id;
    }
    *item = message;
    true
}

fn is_synthetic_instruction(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with("<codex_delegation>")
        || trimmed.starts_with("<heartbeat>")
        || trimmed.starts_with("<automation>")
        || trimmed.starts_with("<automation_update>")
}

fn tool_output_text(output: Option<&Value>) -> Option<String> {
    match output {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        Some(Value::Object(obj)) => obj
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| serde_json::to_string(obj).ok()),
        Some(Value::Array(_)) => serde_json::to_string(output?).ok(),
        _ => None,
    }
}

fn json_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_agent_message_with_encrypted_content_to_user_message() {
        let mut body = json!({
            "input": [
                {
                    "type": "agent_message",
                    "id": "amsg_child",
                    "author": "/root",
                    "recipient": "/root/worker",
                    "content": [
                        {"type": "input_text", "text": "Message Type: NEW_TASK\nPayload:\n"},
                        {"type": "encrypted_content", "encrypted_content": "Review the diff."}
                    ]
                }
            ]
        });

        assert!(rewrite_codex_agent_message_input_items(&mut body));
        let item = &body["input"][0];
        assert_eq!(item["type"], "message");
        assert_eq!(item["role"], "user");
        assert_eq!(item["id"], "amsg_child");
        assert_eq!(
            item["content"][0]["text"],
            "Message Type: NEW_TASK\nPayload:\n"
        );
        assert_eq!(item["content"][1]["text"], "Review the diff.");
        assert!(item.get("author").is_none());
        assert!(item.get("recipient").is_none());
        assert!(!rewrite_codex_agent_message_input_items(&mut body));
    }

    #[test]
    fn leaves_function_call_output_with_call_id_untouched() {
        let mut body = json!({
            "input": [
                {"type": "function_call", "call_id": "c1", "name": "read_file"},
                {"type": "function_call_output", "call_id": "c1", "output": "file content"}
            ]
        });
        let original = body.clone();

        assert!(!rewrite_standalone_function_call_outputs(&mut body));
        assert_eq!(body, original);
    }

    #[test]
    fn rewrites_standalone_delegation_output_without_call_id() {
        let mut body = json!({
            "input": [
                {
                    "type": "function_call_output",
                    "id": "fco_delegation",
                    "name": "create_thread",
                    "namespace": "codex_app",
                    "output": "<codex_delegation>\n  <source_thread_id>src-1</source_thread_id>\n  <input>Count README files.</input>\n</codex_delegation>"
                }
            ]
        });

        assert!(rewrite_standalone_function_call_outputs(&mut body));
        let item = &body["input"][0];
        assert_eq!(item["type"], "message");
        assert_eq!(item["role"], "user");
        assert_eq!(item["id"], "fco_delegation");
        assert!(item.get("name").is_none());
        assert!(item.get("namespace").is_none());
        assert!(item["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("<codex_delegation>"));
        assert!(!rewrite_standalone_function_call_outputs(&mut body));
    }

    #[test]
    fn rewrites_standalone_delegation_with_object_output_text() {
        let mut body = json!({
            "input": [
                {
                    "type": "function_call_output",
                    "name": "create_thread",
                    "namespace": "codex_app",
                    "output": {
                        "text": "<codex_delegation>object task</codex_delegation>"
                    }
                }
            ]
        });

        assert!(rewrite_standalone_function_call_outputs(&mut body));
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert!(body["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("<codex_delegation>"));
    }

    #[test]
    fn rewrites_heartbeat_output_without_call_id() {
        let mut body = json!({
            "input": [
                {
                    "type": "function_call_output",
                    "name": "automation_update",
                    "namespace": "codex_app",
                    "output": "<heartbeat>\n  <instructions>Check the status.</instructions>\n</heartbeat>"
                }
            ]
        });

        assert!(rewrite_standalone_function_call_outputs(&mut body));
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
    }

    #[test]
    fn leaves_plain_missing_call_id_output_untouched() {
        let mut body = json!({
            "input": [
                {
                    "type": "function_call_output",
                    "output": "plain tool output without call id"
                }
            ]
        });
        let original = body.clone();

        assert!(!rewrite_standalone_function_call_outputs(&mut body));
        assert_eq!(body, original);
    }

    #[test]
    fn apply_compat_rewrites_both_synthetic_items() {
        let mut body = json!({
            "input": [
                {
                    "type": "agent_message",
                    "content": [{"type": "input_text", "text": "NEW_TASK"}]
                },
                {
                    "type": "function_call_output",
                    "name": "send_message_to_thread",
                    "output": "<codex_delegation>do it</codex_delegation>"
                }
            ]
        });

        assert!(apply_codex_third_party_request_compat(
            &mut body, "deepseek"
        ));
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][1]["type"], "message");
        assert_eq!(body["input"][1]["role"], "user");
    }

    #[test]
    fn rewrites_nested_synthetic_items() {
        let mut body = json!({
            "input": {
                "collaboration": {
                    "items": [
                        {
                            "type": "agent_message",
                            "content": [{"type": "input_text", "text": "nested task"}]
                        },
                        {
                            "type": "function_call_output",
                            "output": "<codex_delegation>nested delegation</codex_delegation>"
                        }
                    ]
                }
            }
        });

        assert!(apply_codex_third_party_request_compat(&mut body, "kimi"));
        assert_eq!(
            body["input"]["collaboration"]["items"][0]["type"],
            "message"
        );
        assert_eq!(
            body["input"]["collaboration"]["items"][1]["type"],
            "message"
        );
    }
}
