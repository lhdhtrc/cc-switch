//! Read-only Antigravity quota. No Code Assist onboarding or model generation.

use super::subscription::{CredentialStatus, QuotaTier, SubscriptionQuota};
use serde::Deserialize;
use std::collections::BTreeMap;

// Gemini quota is client-profile dependent. Daily with the CLI profile returns
// the current Gemini and Claude/GPT windows; production can return legacy limits.
const BASES: [&str; 1] = ["https://daily-cloudcode-pa.googleapis.com"];

fn user_agent() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    };
    // This is the verified API client profile version, not the cc-switch version.
    format!("antigravity/cli/1.4.3 (aidev_client; os_type={os}; arch={arch}; auth_method=consumer)")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    #[serde(default)]
    groups: Vec<Group>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    #[serde(alias = "display_name")]
    display_name: Option<String>,
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    #[serde(alias = "bucket_id")]
    bucket_id: Option<String>,
    window: Option<String>,
    #[serde(alias = "display_name")]
    display_name: Option<String>,
    #[serde(alias = "remaining_fraction")]
    remaining_fraction: Option<f64>,
    #[serde(alias = "reset_time")]
    reset_time: Option<String>,
    remaining: Option<RemainingQuota>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemainingQuota {
    #[serde(alias = "remaining_fraction")]
    remaining_fraction: Option<f64>,
    #[serde(alias = "reset_time")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
struct Models {
    #[serde(default)]
    models: BTreeMap<String, Model>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    display_name: Option<String>,
    quota_info: Option<ModelQuota>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelQuota {
    remaining_fraction: Option<f64>,
    reset_time: Option<String>,
}

enum QueryError {
    Expired,
    Unavailable,
    Rejected(String),
    Transient(String),
}

fn error(status: CredentialStatus, message: &str) -> SubscriptionQuota {
    SubscriptionQuota::error("antigravity", status, message.to_string())
}

fn result(tiers: Vec<QuotaTier>) -> SubscriptionQuota {
    if tiers.is_empty() {
        return error(
            CredentialStatus::Valid,
            "反重力未返回可用配额数据，请在官方 Antigravity 确认账号状态后刷新。",
        );
    }
    SubscriptionQuota {
        tool: "antigravity".to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage: None,
        credits_balance: None,
        reset_credits: None,
        error: None,
        queried_at: Some(chrono::Utc::now().timestamp_millis()),
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn tier(name: String, remaining: Option<f64>, reset: Option<String>) -> Option<QuotaTier> {
    let remaining = remaining.filter(|value| value.is_finite())?;
    Some(QuotaTier {
        name,
        utilization: (1.0 - remaining.clamp(0.0, 1.0)) * 100.0,
        resets_at: nonempty(reset),
        used_value_usd: None,
        max_value_usd: None,
    })
}

fn normalize_window(bucket: &Bucket, index: usize) -> (String, u8) {
    let id_str = bucket.bucket_id.as_deref().unwrap_or("").to_lowercase();
    let win_str = bucket.window.as_deref().unwrap_or("").to_lowercase();
    let disp_str = bucket.display_name.as_deref().unwrap_or("").to_lowercase();

    // 优先检测 5 小时限额（Short window）
    if win_str == "5h"
        || win_str.contains("5h")
        || win_str.contains("five_hour")
        || id_str.contains("5h")
        || disp_str.contains("five hour")
        || disp_str.contains("5 hour")
        || disp_str.contains("5h")
    {
        return ("5h".to_string(), 0);
    }

    // 其次检测周限额（Weekly window）
    if win_str == "weekly"
        || win_str.contains("week")
        || win_str.contains("7d")
        || win_str.contains("seven_day")
        || id_str.contains("weekly")
        || id_str.contains("week")
        || disp_str.contains("weekly")
        || disp_str.contains("week")
    {
        return ("weekly".to_string(), 1);
    }

    let fallback = nonempty(bucket.window.clone())
        .or_else(|| nonempty(bucket.bucket_id.clone()))
        .or_else(|| nonempty(bucket.display_name.clone()))
        .unwrap_or_else(|| format!("Quota {}", index + 1));
    (fallback, 2)
}

fn summary_quota(summary: Summary) -> SubscriptionQuota {
    let mut tiers = Vec::new();
    for (group_idx, group) in summary.groups.into_iter().enumerate() {
        let group_name = nonempty(group.display_name)
            .unwrap_or_else(|| format!("Antigravity {}", group_idx + 1));
        let mut buckets_with_meta: Vec<_> = group
            .buckets
            .into_iter()
            .enumerate()
            .map(|(b_idx, bucket)| {
                let (win, sort_order) = normalize_window(&bucket, b_idx);
                (sort_order, win, bucket)
            })
            .collect();
        // 保证短周期（5h）排在长周期（weekly）前面
        buckets_with_meta.sort_by_key(|(order, _, _)| *order);

        for (_, window, bucket) in buckets_with_meta {
            // New responses can wrap the balance in `remaining`. Do not let a
            // stale flat fraction override it, including when the balance is 0.
            let (remaining, reset) = match bucket.remaining {
                Some(value) => (
                    value.remaining_fraction,
                    nonempty(value.reset_time).or(bucket.reset_time),
                ),
                None => (bucket.remaining_fraction, bucket.reset_time),
            };
            if let Some(value) = tier(format!("{group_name} / {window}"), remaining, reset) {
                tiers.push(value);
            }
        }
    }
    result(tiers)
}

fn model_quota(models: Models) -> SubscriptionQuota {
    let tiers = models
        .models
        .into_iter()
        .filter_map(|(id, model)| {
            let quota = model.quota_info?;
            let label = nonempty(model.display_name).unwrap_or_else(|| id.clone());
            // Include the ID so distinct models with the same display name remain distinct.
            tier(
                format!("{label} ({id})"),
                quota.remaining_fraction,
                quota.reset_time,
            )
        })
        .collect();
    result(tiers)
}

async fn request(
    token: &str,
    client: &reqwest::Client,
    bases: &[&str],
    action: &str,
) -> Result<serde_json::Value, QueryError> {
    let mut last_error = QueryError::Unavailable;
    for base in bases {
        let response = match client
            .post(format!("{base}/v1internal:{action}"))
            .bearer_auth(token)
            .header(reqwest::header::USER_AGENT, user_agent())
            .json(&serde_json::json!({}))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                last_error = QueryError::Transient(
                    "反重力配额查询网络失败或超时，请检查 Google 网络代理后刷新。".to_string(),
                );
                continue;
            }
        };
        let status = response.status();
        if status.is_success() {
            let bytes = response.bytes().await.map_err(|_| {
                QueryError::Transient("反重力配额响应读取失败，请稍后刷新。".to_string())
            })?;
            return serde_json::from_slice(&bytes).map_err(|_| {
                QueryError::Rejected("反重力配额响应无法解析，请稍后刷新。".to_string())
            });
        }
        match status.as_u16() {
            401 => return Err(QueryError::Expired),
            403 => {
                return Err(QueryError::Rejected(
                    "反重力配额访问被拒绝（HTTP 403），请在官方 Antigravity 确认账号权限或完成验证后刷新。"
                        .to_string(),
                ))
            }
            404 | 501 => {}
            429 | 500..=599 => {
                last_error = QueryError::Transient(format!(
                    "反重力配额服务暂时不可用（HTTP {status}），请稍后刷新。"
                ));
            }
            _ => {
                return Err(QueryError::Rejected(format!(
                    "反重力配额请求失败（HTTP {status}），请在官方 Antigravity 确认账号状态后刷新。"
                )))
            }
        }
    }
    Err(last_error)
}

pub async fn query_quota(token: &str) -> Result<SubscriptionQuota, String> {
    query_quota_at(token, &crate::proxy::http_client::get(), &BASES).await
}

async fn query_quota_at(
    token: &str,
    client: &reqwest::Client,
    bases: &[&str],
) -> Result<SubscriptionQuota, String> {
    let response = request(token, client, bases, "retrieveUserQuotaSummary").await;
    let response = match response {
        Ok(body) => {
            return Ok(match serde_json::from_value(body) {
                Ok(summary) => summary_quota(summary),
                Err(_) => error(
                    CredentialStatus::Valid,
                    "反重力配额汇总响应格式异常，请稍后刷新。",
                ),
            })
        }
        // Only older servers without the summary endpoint may use per-model quota.
        Err(QueryError::Unavailable) => request(token, client, bases, "fetchAvailableModels").await,
        Err(error) => Err(error),
    };
    match response {
        Ok(body) => Ok(match serde_json::from_value(body) {
            Ok(models) => model_quota(models),
            Err(_) => error(
                CredentialStatus::Valid,
                "反重力模型配额响应格式异常，请稍后刷新。",
            ),
        }),
        Err(QueryError::Expired) => Ok(error(
            CredentialStatus::Expired,
            "反重力登录已失效，请在认证中心重新登录。",
        )),
        Err(QueryError::Unavailable) => Ok(error(
            CredentialStatus::Valid,
            "反重力配额接口暂不可用，请在官方 Antigravity 查看配额。",
        )),
        Err(QueryError::Rejected(message)) => Ok(error(CredentialStatus::Valid, &message)),
        Err(QueryError::Transient(message)) => Err(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{StatusCode, Uri},
        routing::post,
        Json, Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    const SUMMARY: &str = r#"{
      "groups": [
        {"displayName": "Gemini Models", "buckets": [
          {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.82,"resetTime":"2026-10-01T00:00:00Z"},
          {"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0}
        ]},
        {"displayName": "Claude and GPT models", "buckets": [
          {"window":"5h","remainingFraction":0.43},
          {"window":"weekly","remainingFraction":0.91},
          {"window":"unknown","resetTime":"2026-10-01T00:00:00Z"}
        ]}
      ]
    }"#;

    #[test]
    fn nested_balances_override_legacy_values_and_keep_both_groups() {
        let quota = summary_quota(
            serde_json::from_str(
                r#"{
          "groups": [
            {"displayName":"Gemini Models","buckets":[
              {"bucketId":"gemini-weekly","displayName":"Weekly Limit Remaining",
               "remainingFraction":0.815,"resetTime":"2026-10-22T01:05:21Z",
               "remaining":{"remainingFraction":0.66,"resetTime":"2026-10-26T00:47:28Z"}},
              {"bucketId":"gemini-5h","displayName":"Five Hour Limit Remaining",
               "remainingFraction":1,"resetTime":"2026-10-20T12:00:00Z",
               "remaining":{"remainingFraction":0.95,"resetTime":"2026-10-20T11:47:24Z"}}
            ]},
            {"displayName":"Claude and GPT models","buckets":[
              {"bucketId":"3p-weekly","window":"weekly","remaining":{"remainingFraction":1}},
              {"bucketId":"3p-5h","window":"5h","remaining":{"remainingFraction":1}}
            ]}
          ]
        }"#,
            )
            .unwrap(),
        );
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 4);
        assert_eq!(quota.tiers[0].name, "Gemini Models / 5h");
        assert!((quota.tiers[0].utilization - 5.0).abs() < 0.001);
        assert_eq!(
            quota.tiers[0].resets_at.as_deref(),
            Some("2026-10-20T11:47:24Z")
        );
        assert_eq!(quota.tiers[1].name, "Gemini Models / weekly");
        assert!((quota.tiers[1].utilization - 34.0).abs() < 0.001);
        assert_eq!(
            quota.tiers[1].resets_at.as_deref(),
            Some("2026-10-26T00:47:28Z")
        );
        assert_eq!(quota.tiers[2].name, "Claude and GPT models / 5h");
        assert_eq!(quota.tiers[3].name, "Claude and GPT models / weekly");
        assert_eq!(quota.tiers[2].utilization, 0.0);
        assert_eq!(quota.tiers[3].utilization, 0.0);
    }

    #[test]
    fn nested_zero_is_exhausted_and_missing_balance_never_uses_stale_fraction() {
        let quota = summary_quota(
            serde_json::from_str(
                r#"{
          "groups":[{"displayName":"Gemini Models","buckets":[
            {"window":"5h","remainingFraction":1,"resetTime":"2026-10-20T11:47:24Z",
             "remaining":{"remainingFraction":0}},
            {"window":"weekly","remainingFraction":1,"remaining":{}},
            {"window":"future","remainingFraction":1,"remaining":{"remainingFraction":null}}
          ]}]
        }"#,
            )
            .unwrap(),
        );
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].utilization, 100.0);
        assert_eq!(
            quota.tiers[0].resets_at.as_deref(),
            Some("2026-10-20T11:47:24Z")
        );
    }

    #[test]
    fn snake_case_balances_and_labels_are_compatible() {
        let quota = summary_quota(
            serde_json::from_str(
                r#"{
          "groups":[{"display_name":"Gemini Models","buckets":[
            {"bucket_id":"gemini-weekly","display_name":"Weekly Limit Remaining",
             "remaining_fraction":0.5,"reset_time":"2026-10-26T00:47:28Z"},
            {"bucket_id":"gemini-5h","display_name":"Five Hour Limit Remaining",
             "remaining":{"remaining_fraction":0.9,"reset_time":"2026-10-20T11:47:24Z"}}
          ]}]
        }"#,
            )
            .unwrap(),
        );
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 2);
        assert_eq!(quota.tiers[0].name, "Gemini Models / 5h");
        assert!((quota.tiers[0].utilization - 10.0).abs() < 0.001);
        assert_eq!(quota.tiers[1].name, "Gemini Models / weekly");
        assert_eq!(quota.tiers[1].utilization, 50.0);
    }

    #[test]
    fn orders_short_window_before_weekly_even_if_upstream_reversed() {
        let quota = summary_quota(serde_json::from_str(r#"{
          "groups": [
            {"displayName": "Gemini Models", "buckets": [
              {"bucketId":"gemini-weekly","displayName":"Weekly Limit Remaining","remainingFraction":0.72,"resetTime":"2026-10-26T00:47:28Z"},
              {"bucketId":"gemini-5h","displayName":"Five Hour Limit Remaining","remainingFraction":0.90,"resetTime":"2026-10-20T06:47:24Z"}
            ]}
          ]
        }"#).unwrap());
        assert_eq!(quota.tiers.len(), 2);
        assert_eq!(quota.tiers[0].name, "Gemini Models / 5h");
        assert!((quota.tiers[0].utilization - 10.0).abs() < 0.001);
        assert_eq!(quota.tiers[1].name, "Gemini Models / weekly");
        assert!((quota.tiers[1].utilization - 28.0).abs() < 0.001);
    }

    #[test]
    fn preserves_groups_windows_exhaustion_and_reset() {
        let quota = summary_quota(serde_json::from_str(SUMMARY).unwrap());
        assert_eq!(quota.tool, "antigravity");
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 4);
        assert_eq!(quota.tiers[0].name, "Gemini Models / 5h");
        assert!((quota.tiers[0].utilization - 18.0).abs() < 0.001);
        assert_eq!(
            quota.tiers[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(quota.tiers[1].utilization, 100.0);
        assert_eq!(quota.tiers[2].name, "Claude and GPT models / 5h");
    }

    #[test]
    fn never_invents_remaining_quota_or_windows() {
        for body in [
            r#"{}"#,
            r#"{"groups":[]}"#,
            r#"{"groups":[{"buckets":[{"remainingFraction":null}]}]}"#,
        ] {
            assert!(!summary_quota(serde_json::from_str(body).unwrap()).success);
        }
        let quota = summary_quota(serde_json::from_str(
            r#"{"groups":[{"displayName":"Future models","buckets":[{"window":"monthly","remainingFraction":0.7}]}]}"#
        ).unwrap());
        assert_eq!(quota.tiers[0].name, "Future models / monthly");
    }

    #[test]
    fn legacy_model_quota_preserves_claude_and_unknown_models() {
        let quota = model_quota(
            serde_json::from_str(
                r#"{"models":{
          "gemini-pro":{"displayName":"Gemini Pro","quotaInfo":{"remainingFraction":0.8}},
          "claude-opus":{"displayName":"Claude Opus","quotaInfo":{"remainingFraction":0}},
          "future-model":{"quotaInfo":{"remainingFraction":0.5}},
          "no-quota":{"quotaInfo":{}}
        }}"#,
            )
            .unwrap(),
        );
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 3);
        assert_eq!(quota.tiers[0].name, "Claude Opus (claude-opus)");
        assert_eq!(quota.tiers[0].utilization, 100.0);
        assert!(!model_quota(serde_json::from_str(r#"{"models":{}}"#).unwrap()).success);
    }

    #[test]
    fn clamps_invalid_bounds_but_keeps_zero() {
        assert_eq!(
            tier("a".into(), Some(-1.0), None).unwrap().utilization,
            100.0
        );
        assert_eq!(tier("a".into(), Some(2.0), None).unwrap().utilization, 0.0);
        assert!(tier("a".into(), Some(f64::NAN), None).is_none());
    }

    async fn mock(
        status: StatusCode,
        body: serde_json::Value,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/:action",
                post(
                    move |State(calls): State<Arc<AtomicUsize>>,
                          uri: Uri,
                          headers: axum::http::HeaderMap,
                          Json(payload): Json<serde_json::Value>| {
                        let body = body.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer test-token");
                            assert!(headers["user-agent"]
                                .to_str()
                                .unwrap()
                                .starts_with("antigravity/cli/"));
                            assert!(headers["user-agent"]
                                .to_str()
                                .unwrap()
                                .contains("auth_method=consumer"));
                            assert_eq!(payload, serde_json::json!({}));
                            match uri.path() {
                                "/v1internal:retrieveUserQuotaSummary" => (status, Json(body)),
                                "/v1internal:fetchAvailableModels" => {
                                    calls.fetch_add(1, Ordering::SeqCst);
                                    (
                                        StatusCode::OK,
                                        Json(serde_json::json!({"models": {
                                            "claude-opus":{"quotaInfo":{"remainingFraction":0.3}}
                                        }})),
                                    )
                                }
                                _ => (StatusCode::BAD_REQUEST, Json(serde_json::json!({}))),
                            }
                        }
                    },
                ),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, calls, server)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    #[tokio::test]
    async fn queries_summary_without_cli_project_or_onboarding() {
        let (base, calls, server) =
            mock(StatusCode::OK, serde_json::from_str(SUMMARY).unwrap()).await;
        let quota = query_quota_at("test-token", &client(), &[&base])
            .await
            .unwrap();
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 4);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn distinguishes_unauthorized_forbidden_and_empty_summary() {
        for (status, body, expired) in [
            (
                StatusCode::UNAUTHORIZED,
                serde_json::json!({"secret":"do-not-echo"}),
                true,
            ),
            (
                StatusCode::FORBIDDEN,
                serde_json::json!({"secret":"do-not-echo"}),
                false,
            ),
            (StatusCode::OK, serde_json::json!({"groups":[]}), false),
        ] {
            let (base, calls, server) = mock(status, body).await;
            let quota = query_quota_at("test-token", &client(), &[&base])
                .await
                .unwrap();
            assert!(!quota.success);
            assert_eq!(
                matches!(quota.credential_status, CredentialStatus::Expired),
                expired
            );
            assert!(!quota.error.unwrap().contains("do-not-echo"));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            server.abort();
        }
    }

    #[tokio::test]
    async fn legacy_fallback_only_for_missing_summary_endpoint() {
        for status in [StatusCode::NOT_FOUND, StatusCode::NOT_IMPLEMENTED] {
            let (base, calls, server) = mock(status, serde_json::json!({})).await;
            let quota = query_quota_at("test-token", &client(), &[&base])
                .await
                .unwrap();
            assert!(quota.success);
            assert_eq!(quota.tiers[0].name, "claude-opus (claude-opus)");
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }

    #[tokio::test]
    async fn transient_failure_does_not_use_misleading_model_quota() {
        let (base, calls, server) =
            mock(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({})).await;
        assert!(query_quota_at("test-token", &client(), &[&base])
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn uses_daily_cli_profile_without_legacy_production_fallback() {
        assert_eq!(BASES, ["https://daily-cloudcode-pa.googleapis.com"]);
        let (daily, calls, server) =
            mock(StatusCode::OK, serde_json::from_str(SUMMARY).unwrap()).await;
        let quota = query_quota_at("test-token", &client(), &[&daily])
            .await
            .unwrap();
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 4);
        assert_eq!(quota.tiers[0].name, "Gemini Models / 5h");
        assert_eq!(quota.tiers[1].name, "Gemini Models / weekly");
        assert_eq!(quota.tiers[2].name, "Claude and GPT models / 5h");
        assert_eq!(quota.tiers[3].name, "Claude and GPT models / weekly");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn quota_server_errors_remain_retryable_instead_of_using_legacy_limits() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let (base, calls, server) = mock(status, serde_json::json!({})).await;
            assert!(query_quota_at("test-token", &client(), &[&base])
                .await
                .is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            server.abort();
        }
    }

    #[tokio::test]
    async fn malformed_or_denied_summary_never_falls_back_to_model_catalog() {
        for (status, body) in [
            (StatusCode::OK, serde_json::json!({"groups": "invalid"})),
            (
                StatusCode::BAD_REQUEST,
                serde_json::json!({"token":"do-not-echo"}),
            ),
        ] {
            let (base, calls, server) = mock(status, body).await;
            let quota = query_quota_at("test-token", &client(), &[&base])
                .await
                .unwrap();
            assert!(!quota.success);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(!quota.error.unwrap().contains("do-not-echo"));
            server.abort();
        }
    }
}
