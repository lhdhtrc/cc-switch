//! Quota-only Antigravity OAuth accounts. Never writes official client credentials or routes.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{oneshot, watch, Mutex};
use uuid::Uuid;

const MISSING_OAUTH_CONFIG_MESSAGE: &str =
    "当前构建未配置反重力 OAuth 客户端参数，请使用已注入参数的构建或联系维护者。";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const SCOPES: &str = "openid https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClientCredentials {
    client_id: String,
    client_secret: String,
}

fn resolve_client_credentials(
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<ClientCredentials, String> {
    let client_id = client_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| MISSING_OAUTH_CONFIG_MESSAGE.to_string())?;
    let client_secret = client_secret
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| MISSING_OAUTH_CONFIG_MESSAGE.to_string())?;
    Ok(ClientCredentials {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
    })
}

#[derive(Clone, Serialize, Deserialize)]
struct AccountData {
    id: String,
    email: String,
    authenticated_at: i64,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    revision: String,
    #[serde(default)]
    requires_reauth: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct AccountStore {
    accounts: BTreeMap<String, AccountData>,
    default_account_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityAccount {
    id: String,
    email: String,
    authenticated_at: i64,
    is_default: bool,
    requires_reauth: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityAuthStatus {
    accounts: Vec<AntigravityAccount>,
    default_account_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityLogin {
    pub flow_id: String,
    pub authorization_url: String,
    expires_at: i64,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AntigravityLoginResult {
    Pending,
    Completed { account: AntigravityAccount },
    Error { message: String },
}

struct PendingLogin {
    id: String,
    cancel: watch::Sender<bool>,
    result: AntigravityLoginResult,
}

struct Inner {
    store: AccountStore,
    load_error: Option<String>,
    pending: Option<PendingLogin>,
}

#[derive(Clone)]
struct Endpoints {
    token: String,
    userinfo: String,
}

pub struct AntigravityOAuthManager {
    storage_path: PathBuf,
    inner: Mutex<Inner>,
    refresh_lock: Mutex<()>,
    endpoints: Endpoints,
    client_credentials: Result<ClientCredentials, String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Deserialize)]
struct UserInfo {
    id: String,
    email: String,
    verified_email: bool,
}

#[derive(Deserialize)]
struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}

#[derive(Clone)]
struct CallbackState {
    state: String,
    sender: Arc<Mutex<Option<oneshot::Sender<Result<String, String>>>>>,
}

async fn callback(
    State(context): State<CallbackState>,
    Query(query): Query<Callback>,
) -> (StatusCode, Html<&'static str>) {
    if query.state.as_deref() != Some(context.state.as_str()) {
        return (StatusCode::BAD_REQUEST, Html("Invalid OAuth state."));
    }
    let result = match (query.code, query.error) {
        (_, Some(_)) => Err("Google 授权被拒绝或取消，请重试。".to_string()),
        (Some(code), None) if !code.is_empty() => Ok(code),
        _ => return (StatusCode::BAD_REQUEST, Html("Missing authorization code.")),
    };
    if let Some(sender) = context.sender.lock().await.take() {
        let _ = sender.send(result);
        (
            StatusCode::OK,
            Html("<!doctype html><meta charset=\"utf-8\"><title>CC Switch</title><p>Authorization received. Return to CC Switch to check the result.</p>"),
        )
    } else {
        (
            StatusCode::CONFLICT,
            Html("This login has already finished."),
        )
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn oauth_url(client_id: &str, state: &str, verifier: &str, redirect: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("response_type", "code")
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent select_account")
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

impl AntigravityOAuthManager {
    pub fn new(config_dir: PathBuf) -> Self {
        let storage_path = config_dir.join("antigravity_oauth_accounts.json");
        let loaded = match std::fs::read(&storage_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "反重力账号文件无法解析，请检查文件后重试。".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AccountStore::default())
            }
            Err(_) => Err("无法读取 反重力账号文件，请检查文件权限。".to_string()),
        };
        let (store, load_error) = match loaded {
            Ok(store) => (store, None),
            Err(error) => (AccountStore::default(), Some(error)),
        };
        Self {
            storage_path,
            inner: Mutex::new(Inner {
                store,
                load_error,
                pending: None,
            }),
            refresh_lock: Mutex::new(()),
            endpoints: Endpoints {
                token: "https://oauth2.googleapis.com/token".to_string(),
                userinfo: "https://www.googleapis.com/oauth2/v2/userinfo".to_string(),
            },
            client_credentials: resolve_client_credentials(
                option_env!("CC_SWITCH_ANTIGRAVITY_CLIENT_ID"),
                option_env!("CC_SWITCH_ANTIGRAVITY_CLIENT_SECRET"),
            ),
        }
    }

    fn client_credentials(&self) -> Result<&ClientCredentials, String> {
        self.client_credentials.as_ref().map_err(Clone::clone)
    }

    fn ensure_loaded(inner: &Inner) -> Result<(), String> {
        inner.load_error.clone().map_or(Ok(()), Err)
    }

    fn summary(data: &AccountData, store: &AccountStore) -> AntigravityAccount {
        AntigravityAccount {
            id: data.id.clone(),
            email: data.email.clone(),
            authenticated_at: data.authenticated_at,
            is_default: store.default_account_id.as_deref() == Some(data.id.as_str()),
            requires_reauth: data.requires_reauth,
        }
    }

    fn save(&self, inner: &mut Inner, store: AccountStore) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&store).map_err(|_| "无法保存 反重力账号。")?;
        crate::config::atomic_write_private(&self.storage_path, &bytes)
            .map_err(|_| "无法保存 反重力账号，请检查文件权限。")?;
        inner.store = store;
        Ok(())
    }

    pub async fn status(&self) -> Result<AntigravityAuthStatus, String> {
        let inner = self.inner.lock().await;
        Self::ensure_loaded(&inner)?;
        Ok(AntigravityAuthStatus {
            accounts: inner
                .store
                .accounts
                .values()
                .map(|data| Self::summary(data, &inner.store))
                .collect(),
            default_account_id: inner.store.default_account_id.clone(),
        })
    }

    pub async fn start_login(
        self: &Arc<Self>,
        target_account_id: Option<String>,
    ) -> Result<AntigravityLogin, String> {
        self.start_login_with_timeout(target_account_id, LOGIN_TIMEOUT)
            .await
    }

    async fn start_login_with_timeout(
        self: &Arc<Self>,
        target: Option<String>,
        timeout: Duration,
    ) -> Result<AntigravityLogin, String> {
        let mut inner = self.inner.lock().await;
        Self::ensure_loaded(&inner)?;
        if let Some(id) = &target {
            if !inner.store.accounts.contains_key(id) {
                return Err("反重力账号不存在。".to_string());
            }
        }
        let credentials = self.client_credentials()?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| "无法启动 Google 登录回调服务。")?;
        let address = listener
            .local_addr()
            .map_err(|_| "无法读取登录回调端口。")?;
        let redirect = format!("http://127.0.0.1:{}/oauth2callback", address.port());
        let state = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let login = AntigravityLogin {
            flow_id: Uuid::new_v4().to_string(),
            authorization_url: oauth_url(&credentials.client_id, &state, &verifier, &redirect),
            expires_at: now_ms() + timeout.as_millis() as i64,
        };
        let (cancel, mut cancelled) = watch::channel(false);
        if let Some(previous) = inner.pending.take() {
            let _ = previous.cancel.send(true);
        }
        inner.pending = Some(PendingLogin {
            id: login.flow_id.clone(),
            cancel,
            result: AntigravityLoginResult::Pending,
        });
        drop(inner);

        let (sender, receiver) = oneshot::channel();
        let router = Router::new()
            .route("/oauth2callback", get(callback))
            .with_state(CallbackState {
                state,
                sender: Arc::new(Mutex::new(Some(sender))),
            });
        let (shutdown, shutdown_requested) = oneshot::channel::<()>();
        let mut server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_requested.await;
                })
                .await
        });
        let manager = self.clone();
        let id = login.flow_id.clone();
        tokio::spawn(async move {
            let work = async {
                let code = receiver
                    .await
                    .map_err(|_| "Google 登录回调已关闭。".to_string())??;
                manager.exchange_code(&code, &verifier, &redirect).await
            };
            let result = tokio::select! {
                _ = cancelled.changed() => None,
                _ = tokio::time::sleep(timeout) => Some(Err("Google 登录已超时，请重试。".to_string())),
                result = work => Some(result),
            };
            if let Some(result) = result {
                manager.complete_login(&id, target.as_deref(), result).await;
            }
            // Let the callback response flush before closing the listener.
            let _ = shutdown.send(());
            if tokio::time::timeout(Duration::from_secs(1), &mut server)
                .await
                .is_err()
            {
                server.abort();
            }
        });
        Ok(login)
    }

    async fn token_request(&self, fields: &[(&str, &str)]) -> Result<TokenResponse, String> {
        let response = crate::proxy::http_client::get()
            .post(&self.endpoints.token)
            .form(fields)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|_| "无法连接 Google 认证服务，请检查网络或全局代理。".to_string())?;
        if !response.status().is_success() {
            let status = response.status();
            let error: serde_json::Value = response.json().await.unwrap_or_default();
            if error.get("error").and_then(|value| value.as_str()) == Some("invalid_grant") {
                return Err("invalid_grant".to_string());
            }
            return Err(format!(
                "Google 认证服务请求失败（HTTP {status}），请重试。"
            ));
        }
        let tokens: TokenResponse = response
            .json()
            .await
            .map_err(|_| "Google 认证响应无法解析。".to_string())?;
        if tokens.access_token.is_empty() || tokens.expires_in <= 0 {
            return Err("Google 返回的登录凭据无效。".to_string());
        }
        Ok(tokens)
    }

    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<AccountData, String> {
        let credentials = self.client_credentials()?;
        let tokens = self
            .token_request(&[
                ("client_id", &credentials.client_id),
                ("client_secret", &credentials.client_secret),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect),
            ])
            .await
            .map_err(|error| {
                if error == "invalid_grant" {
                    "Google 授权码已失效，请重新登录。".to_string()
                } else {
                    error
                }
            })?;
        let response = crate::proxy::http_client::get()
            .get(&self.endpoints.userinfo)
            .bearer_auth(&tokens.access_token)
            .timeout(HTTP_TIMEOUT)
            .send()
            .await
            .map_err(|_| "无法读取 Google 账号信息，请重试。".to_string())?;
        if !response.status().is_success() {
            return Err("读取 Google 账号信息失败，请重试。".to_string());
        }
        let user: UserInfo = response
            .json()
            .await
            .map_err(|_| "Google 账号信息无法解析。")?;
        if user.id.is_empty() || user.email.is_empty() || !user.verified_email {
            return Err("Google 未返回已验证的账号信息。".to_string());
        }
        let refresh = tokens
            .refresh_token
            .filter(|value| !value.is_empty())
            .ok_or("Google 未授予离线访问，请重新登录并允许授权。")?;
        Ok(AccountData {
            id: user.id,
            email: user.email,
            authenticated_at: now_ms(),
            access_token: tokens.access_token,
            refresh_token: refresh,
            expires_at: now_ms().saturating_add(tokens.expires_in.saturating_mul(1000)),
            revision: Uuid::new_v4().to_string(),
            requires_reauth: false,
        })
    }

    async fn complete_login(
        &self,
        flow_id: &str,
        target: Option<&str>,
        result: Result<AccountData, String>,
    ) {
        let mut inner = self.inner.lock().await;
        if !inner
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == flow_id)
        {
            return;
        }
        let result = result.and_then(|account| {
            if let Some(target) = target {
                if account.id != target || !inner.store.accounts.contains_key(target) {
                    return Err("重新登录的 Google 账号与原账号不一致，请选择原账号。".to_string());
                }
            }
            let mut store = inner.store.clone();
            store.accounts.insert(account.id.clone(), account.clone());
            if store.default_account_id.is_none() {
                store.default_account_id = Some(account.id.clone());
            }
            self.save(&mut inner, store)?;
            Ok(Self::summary(&account, &inner.store))
        });
        if let Some(pending) = &mut inner.pending {
            pending.result = match result {
                Ok(account) => AntigravityLoginResult::Completed { account },
                Err(message) => AntigravityLoginResult::Error { message },
            };
        }
    }

    pub async fn poll_login(&self, flow_id: &str) -> AntigravityLoginResult {
        self.inner
            .lock()
            .await
            .pending
            .as_ref()
            .filter(|pending| pending.id == flow_id)
            .map(|pending| pending.result.clone())
            .unwrap_or(AntigravityLoginResult::Error {
                message: "Google 登录已取消或被新的登录替代。".to_string(),
            })
    }

    pub async fn cancel_login(&self, flow_id: &str) {
        let mut inner = self.inner.lock().await;
        if inner
            .pending
            .as_ref()
            .is_some_and(|pending| pending.id == flow_id)
        {
            if let Some(pending) = inner.pending.take() {
                let _ = pending.cancel.send(true);
            }
        }
    }

    pub async fn remove_account(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        Self::ensure_loaded(&inner)?;
        let mut store = inner.store.clone();
        if store.accounts.remove(id).is_none() {
            return Err("反重力账号不存在。".to_string());
        }
        if store.default_account_id.as_deref() == Some(id) {
            store.default_account_id = store.accounts.keys().next().cloned();
        }
        self.save(&mut inner, store)?;
        if let Some(pending) = inner.pending.take() {
            let _ = pending.cancel.send(true);
        }
        Ok(())
    }

    pub async fn set_default(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        Self::ensure_loaded(&inner)?;
        if !inner.store.accounts.contains_key(id) {
            return Err("反重力账号不存在。".to_string());
        }
        let mut store = inner.store.clone();
        store.default_account_id = Some(id.to_string());
        self.save(&mut inner, store)
    }

    pub async fn access_token(&self, id: &str) -> Result<String, String> {
        // Serialize refreshes so simultaneous quota requests never overwrite rotated tokens.
        let _refresh = self.refresh_lock.lock().await;
        let account = {
            let inner = self.inner.lock().await;
            Self::ensure_loaded(&inner)?;
            inner
                .store
                .accounts
                .get(id)
                .cloned()
                .ok_or("反重力账号不存在。")?
        };
        if account.requires_reauth {
            return Err("invalid_grant".to_string());
        }
        if account.expires_at > now_ms() + 60_000 {
            return Ok(account.access_token);
        }
        let credentials = self.client_credentials()?;
        let result = self
            .token_request(&[
                ("client_id", &credentials.client_id),
                ("client_secret", &credentials.client_secret),
                ("grant_type", "refresh_token"),
                ("refresh_token", &account.refresh_token),
            ])
            .await;
        let mut inner = self.inner.lock().await;
        let current = inner.store.accounts.get(id).ok_or("反重力账号已移除。")?;
        if current.revision != account.revision {
            return Err("反重力登录状态已变化，请刷新配额。".to_string());
        }
        let mut store = inner.store.clone();
        let data = store.accounts.get_mut(id).unwrap();
        match result {
            Ok(tokens) => {
                data.access_token = tokens.access_token.clone();
                data.expires_at = now_ms().saturating_add(tokens.expires_in.saturating_mul(1000));
                if let Some(refresh) = tokens.refresh_token.filter(|value| !value.is_empty()) {
                    data.refresh_token = refresh;
                }
                data.revision = Uuid::new_v4().to_string();
                self.save(&mut inner, store)?;
                Ok(tokens.access_token)
            }
            Err(error) => {
                if error == "invalid_grant" {
                    data.requires_reauth = true;
                    self.save(&mut inner, store)?;
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Form, Json};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_CLIENT_ID: &str = "test-antigravity-client-id.apps.googleusercontent.com";
    const TEST_CLIENT_SECRET: &str = "test-antigravity-client-secret";

    fn manager_with_credentials(
        config_dir: PathBuf,
        client_id: Option<&str>,
        client_secret: Option<&str>,
    ) -> AntigravityOAuthManager {
        let mut manager = AntigravityOAuthManager::new(config_dir);
        manager.client_credentials = resolve_client_credentials(client_id, client_secret);
        manager
    }

    fn test_manager(config_dir: PathBuf) -> AntigravityOAuthManager {
        manager_with_credentials(config_dir, Some(TEST_CLIENT_ID), Some(TEST_CLIENT_SECRET))
    }

    fn account(id: &str) -> AccountData {
        AccountData {
            id: id.to_string(),
            email: format!("{id}@example.test"),
            authenticated_at: now_ms(),
            access_token: "cached-access".to_string(),
            refresh_token: "old-refresh".to_string(),
            expires_at: now_ms() + 3_600_000,
            revision: Uuid::new_v4().to_string(),
            requires_reauth: false,
        }
    }

    async fn seed(manager: &AntigravityOAuthManager, data: AccountData) {
        let mut inner = manager.inner.lock().await;
        let mut store = inner.store.clone();
        if store.default_account_id.is_none() {
            store.default_account_id = Some(data.id.clone());
        }
        store.accounts.insert(data.id.clone(), data);
        manager.save(&mut inner, store).unwrap();
    }

    async fn fake_google(
        config_dir: PathBuf,
        reject_refresh: bool,
    ) -> (
        Arc<AntigravityOAuthManager>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        let token_calls = calls.clone();
        let router = Router::new()
            .route("/token", post(move |Form(fields): Form<BTreeMap<String, String>>| {
                let calls = token_calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(fields["client_id"], TEST_CLIENT_ID);
                    assert_eq!(fields["client_secret"], TEST_CLIENT_SECRET);
                    if fields["grant_type"] == "refresh_token" && reject_refresh {
                        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid_grant"})));
                    }
                    if fields["grant_type"] == "authorization_code" {
                        assert_eq!(fields["code_verifier"].len(), 64);
                        assert!(fields["redirect_uri"].starts_with("http://127.0.0.1:"));
                    }
                    (StatusCode::OK, Json(serde_json::json!({
                        "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 3600
                    })))
                }
            }))
            .route("/userinfo", get(|| async {
                Json(serde_json::json!({"id": "google-id", "email": "user@example.test", "verified_email": true}))
            }));
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut manager = test_manager(config_dir);
        manager.endpoints = Endpoints {
            token: format!("{base}/token"),
            userinfo: format!("{base}/userinfo"),
        };
        (Arc::new(manager), calls, server)
    }

    fn callback_url(login: &AntigravityLogin, valid_state: bool, denied: bool) -> String {
        let auth = url::Url::parse(&login.authorization_url).unwrap();
        let params: BTreeMap<_, _> = auth.query_pairs().into_owned().collect();
        let mut callback = url::Url::parse(&params["redirect_uri"]).unwrap();
        callback
            .query_pairs_mut()
            .append_pair(
                "state",
                if valid_state {
                    &params["state"]
                } else {
                    "wrong-state"
                },
            )
            .append_pair(
                if denied { "error" } else { "code" },
                if denied { "access_denied" } else { "test-code" },
            );
        callback.into()
    }

    async fn send_callback(
        login: &AntigravityLogin,
        valid_state: bool,
        denied: bool,
    ) -> StatusCode {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(callback_url(login, valid_state, denied))
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn completed(
        manager: &AntigravityOAuthManager,
        login: &AntigravityLogin,
    ) -> AntigravityLoginResult {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let result = manager.poll_login(&login.flow_id).await;
                if !matches!(result, AntigravityLoginResult::Pending) {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    #[test]
    fn authorization_url_has_pkce_offline_access_and_correct_scopes() {
        let verifier = "a".repeat(64);
        let url = url::Url::parse(&oauth_url(
            TEST_CLIENT_ID,
            "test-state",
            &verifier,
            "http://127.0.0.1:1234/oauth2callback",
        ))
        .unwrap();
        let fields: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("accounts.google.com"));
        assert_eq!(fields["state"], "test-state");
        assert_eq!(fields["access_type"], "offline");
        assert_eq!(fields["client_id"], TEST_CLIENT_ID);
        assert!(fields["scope"].contains("/auth/cclog"));
        assert!(fields["scope"].contains("/auth/experimentsandconfigs"));
        assert_eq!(fields["code_challenge_method"], "S256");
        assert_eq!(
            fields["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
        assert_eq!(fields["scope"], SCOPES);
        assert!(!fields.contains_key("client_secret"));
    }

    #[tokio::test]
    async fn never_loads_or_overwrites_legacy_gemini_cli_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("gemini_oauth_accounts.json");
        let bytes = b"legacy-client-credentials-must-not-be-read";
        std::fs::write(&legacy, bytes).unwrap();
        let manager = AntigravityOAuthManager::new(dir.path().to_path_buf());
        assert!(manager.status().await.unwrap().accounts.is_empty());
        assert_eq!(
            manager.storage_path,
            dir.path().join("antigravity_oauth_accounts.json")
        );
        assert_eq!(std::fs::read(&legacy).unwrap(), bytes);
    }

    #[tokio::test]
    async fn browser_callback_rejects_wrong_state_and_persists_only_valid_login() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, calls, server) = fake_google(dir.path().to_path_buf(), false).await;
        let login = manager.start_login(None).await.unwrap();
        assert_eq!(
            send_callback(&login, false, false).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            manager.poll_login(&login.flow_id).await,
            AntigravityLoginResult::Pending
        ));
        assert_eq!(send_callback(&login, true, false).await, StatusCode::OK);
        assert!(matches!(
            completed(&manager, &login).await,
            AntigravityLoginResult::Completed { .. }
        ));
        let status = manager.status().await.unwrap();
        assert_eq!(status.accounts.len(), 1);
        assert_eq!(status.accounts[0].email, "user@example.test");
        assert_eq!(status.default_account_id.as_deref(), Some("google-id"));
        let public = serde_json::to_string(&status).unwrap();
        assert!(!public.contains("new-access"));
        assert!(!public.contains("new-refresh"));
        let reloaded = test_manager(dir.path().to_path_buf());
        assert_eq!(reloaded.status().await.unwrap().accounts.len(), 1);
        assert_eq!(
            reloaded.access_token("google-id").await.unwrap(),
            "new-access"
        );
        assert!(!dir.path().join("oauth_creds.json").exists());
        server.abort();
    }

    #[tokio::test]
    async fn denied_callback_never_creates_account_or_exchanges_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, calls, server) = fake_google(dir.path().to_path_buf(), false).await;
        let login = manager.start_login(None).await.unwrap();
        assert_eq!(send_callback(&login, true, true).await, StatusCode::OK);
        assert!(matches!(
            completed(&manager, &login).await,
            AntigravityLoginResult::Error { .. }
        ));
        assert!(manager.status().await.unwrap().accounts.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn cancel_and_removed_account_cannot_be_resurrected_by_late_completion() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(test_manager(dir.path().to_path_buf()));
        let login = manager.start_login(None).await.unwrap();
        manager.cancel_login(&login.flow_id).await;
        manager
            .complete_login(&login.flow_id, None, Ok(account("late")))
            .await;
        assert!(manager.status().await.unwrap().accounts.is_empty());
        seed(&manager, account("removed")).await;
        let login = manager
            .start_login(Some("removed".to_string()))
            .await
            .unwrap();
        manager.remove_account("removed").await.unwrap();
        manager
            .complete_login(&login.flow_id, Some("removed"), Ok(account("removed")))
            .await;
        assert!(manager.status().await.unwrap().accounts.is_empty());
    }

    #[tokio::test]
    async fn newest_login_wins_and_cancelling_old_flow_does_not_cancel_new_flow() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(test_manager(dir.path().to_path_buf()));
        let old = manager.start_login(None).await.unwrap();
        let new = manager.start_login(None).await.unwrap();
        manager.cancel_login(&old.flow_id).await;
        manager
            .complete_login(&old.flow_id, None, Ok(account("old")))
            .await;
        assert!(matches!(
            manager.poll_login(&new.flow_id).await,
            AntigravityLoginResult::Pending
        ));
        manager
            .complete_login(&new.flow_id, None, Ok(account("new")))
            .await;
        assert_eq!(manager.status().await.unwrap().accounts[0].id, "new");
        manager.cancel_login(&new.flow_id).await;
    }

    #[tokio::test]
    async fn timeout_is_reported_and_does_not_create_account() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(test_manager(dir.path().to_path_buf()));
        let login = manager
            .start_login_with_timeout(None, Duration::from_millis(20))
            .await
            .unwrap();
        assert!(matches!(
            completed(&manager, &login).await,
            AntigravityLoginResult::Error { .. }
        ));
        assert!(manager.status().await.unwrap().accounts.is_empty());
    }

    #[tokio::test]
    async fn reauth_rejects_wrong_account_and_duplicate_login_updates_existing_account() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(test_manager(dir.path().to_path_buf()));
        seed(&manager, account("original")).await;
        let login = manager
            .start_login(Some("original".to_string()))
            .await
            .unwrap();
        manager
            .complete_login(&login.flow_id, Some("original"), Ok(account("other")))
            .await;
        assert!(matches!(
            manager.poll_login(&login.flow_id).await,
            AntigravityLoginResult::Error { .. }
        ));
        assert_eq!(manager.status().await.unwrap().accounts.len(), 1);
        let login = manager.start_login(None).await.unwrap();
        let mut updated = account("original");
        updated.email = "updated@example.test".to_string();
        manager
            .complete_login(&login.flow_id, None, Ok(updated))
            .await;
        let status = manager.status().await.unwrap();
        assert_eq!(status.accounts.len(), 1);
        assert_eq!(status.accounts[0].email, "updated@example.test");
        manager.cancel_login(&login.flow_id).await;
    }

    #[tokio::test]
    async fn refreshes_once_for_concurrent_queries_and_persists_rotated_refresh_token() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, calls, server) = fake_google(dir.path().to_path_buf(), false).await;
        let mut expired = account("expired");
        expired.expires_at = 0;
        seed(&manager, expired).await;
        let (first, second) = tokio::join!(
            manager.access_token("expired"),
            manager.access_token("expired")
        );
        assert_eq!(first.unwrap(), "new-access");
        assert_eq!(second.unwrap(), "new-access");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let store = &manager.inner.lock().await.store;
        assert_eq!(store.accounts["expired"].refresh_token, "new-refresh");
        server.abort();
    }

    #[tokio::test]
    async fn invalid_refresh_marks_only_that_account_for_reauth() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, calls, server) = fake_google(dir.path().to_path_buf(), true).await;
        let mut expired = account("expired");
        expired.expires_at = 0;
        seed(&manager, expired).await;
        seed(&manager, account("healthy")).await;
        assert_eq!(
            manager.access_token("expired").await.unwrap_err(),
            "invalid_grant"
        );
        assert_eq!(
            manager.access_token("expired").await.unwrap_err(),
            "invalid_grant"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            manager.access_token("healthy").await.unwrap(),
            "cached-access"
        );
        let status = manager.status().await.unwrap();
        assert!(
            status
                .accounts
                .iter()
                .find(|data| data.id == "expired")
                .unwrap()
                .requires_reauth
        );
        assert!(
            !status
                .accounts
                .iter()
                .find(|data| data.id == "healthy")
                .unwrap()
                .requires_reauth
        );
        server.abort();
    }

    #[tokio::test]
    async fn default_account_changes_and_removal_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let manager = test_manager(dir.path().to_path_buf());
        seed(&manager, account("first")).await;
        seed(&manager, account("second")).await;
        assert!(manager.set_default("missing").await.is_err());
        manager.set_default("second").await.unwrap();
        assert_eq!(
            manager
                .status()
                .await
                .unwrap()
                .default_account_id
                .as_deref(),
            Some("second")
        );
        manager.remove_account("second").await.unwrap();
        let reloaded = test_manager(dir.path().to_path_buf());
        assert_eq!(
            reloaded
                .status()
                .await
                .unwrap()
                .default_account_id
                .as_deref(),
            Some("first")
        );
        manager.remove_account("first").await.unwrap();
        assert!(manager.status().await.unwrap().default_account_id.is_none());
    }

    #[tokio::test]
    async fn corrupt_storage_is_not_silently_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("antigravity_oauth_accounts.json");
        std::fs::write(&path, b"corrupted").unwrap();
        let manager = Arc::new(test_manager(dir.path().to_path_buf()));
        assert!(manager.status().await.is_err());
        assert!(manager.start_login(None).await.is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"corrupted");
    }

    #[tokio::test]
    async fn missing_or_blank_client_credentials_fail_before_login_or_refresh_without_marking_reauth(
    ) {
        for (client_id, client_secret) in [
            (None, None),
            (Some(TEST_CLIENT_ID), None),
            (None, Some(TEST_CLIENT_SECRET)),
            (Some("   "), Some(TEST_CLIENT_SECRET)),
            (Some(TEST_CLIENT_ID), Some(" \t\n ")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (configured, calls, server) = fake_google(dir.path().to_path_buf(), false).await;
            let mut manager =
                manager_with_credentials(dir.path().to_path_buf(), client_id, client_secret);
            manager.endpoints = configured.endpoints.clone();
            let manager = Arc::new(manager);

            assert_eq!(
                manager.start_login(None).await.err().as_deref(),
                Some(MISSING_OAUTH_CONFIG_MESSAGE)
            );
            assert!(manager.inner.lock().await.pending.is_none());

            seed(&manager, account("cached")).await;
            let mut expired = account("expired");
            expired.expires_at = 0;
            seed(&manager, expired).await;

            let status = manager.status().await.unwrap();
            assert_eq!(status.accounts.len(), 2);
            manager.set_default("expired").await.unwrap();
            assert_eq!(
                manager.access_token("cached").await.unwrap(),
                "cached-access"
            );

            let refresh_err = manager.access_token("expired").await.unwrap_err();
            assert_eq!(refresh_err, MISSING_OAUTH_CONFIG_MESSAGE);
            assert_eq!(calls.load(Ordering::SeqCst), 0);

            let status = manager.status().await.unwrap();
            assert!(
                !status
                    .accounts
                    .iter()
                    .find(|item| item.id == "expired")
                    .unwrap()
                    .requires_reauth
            );

            manager.remove_account("cached").await.unwrap();
            assert_eq!(manager.status().await.unwrap().accounts.len(), 1);
            server.abort();
        }
    }
}
