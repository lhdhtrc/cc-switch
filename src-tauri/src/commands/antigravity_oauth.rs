use std::sync::Arc;
use tauri::State;

use crate::services::antigravity_oauth::{
    AntigravityAuthStatus, AntigravityLogin, AntigravityLoginResult, AntigravityOAuthManager,
};
use crate::services::subscription::{CredentialStatus, SubscriptionQuota};

pub struct AntigravityOAuthState(pub Arc<AntigravityOAuthManager>);

#[tauri::command]
pub async fn antigravity_auth_status(
    state: State<'_, AntigravityOAuthState>,
) -> Result<AntigravityAuthStatus, String> {
    state.0.status().await
}

#[tauri::command]
pub async fn antigravity_auth_start(
    state: State<'_, AntigravityOAuthState>,
    target_account_id: Option<String>,
) -> Result<AntigravityLogin, String> {
    state.0.start_login(target_account_id).await
}

#[tauri::command]
pub async fn antigravity_auth_poll(
    state: State<'_, AntigravityOAuthState>,
    flow_id: String,
) -> Result<AntigravityLoginResult, String> {
    Ok(state.0.poll_login(&flow_id).await)
}

#[tauri::command]
pub async fn antigravity_auth_cancel(
    state: State<'_, AntigravityOAuthState>,
    flow_id: String,
) -> Result<(), String> {
    state.0.cancel_login(&flow_id).await;
    Ok(())
}

#[tauri::command]
pub async fn antigravity_auth_remove(
    state: State<'_, AntigravityOAuthState>,
    account_id: String,
) -> Result<(), String> {
    state.0.remove_account(&account_id).await
}

#[tauri::command]
pub async fn antigravity_auth_set_default(
    state: State<'_, AntigravityOAuthState>,
    account_id: String,
) -> Result<(), String> {
    state.0.set_default(&account_id).await
}

#[tauri::command]
pub async fn get_antigravity_oauth_quota(
    state: State<'_, AntigravityOAuthState>,
    account_id: String,
) -> Result<SubscriptionQuota, String> {
    match state.0.access_token(&account_id).await {
        Ok(token) => {
            let mut quota = crate::services::antigravity_quota::query_quota(&token).await?;
            if matches!(quota.credential_status, CredentialStatus::Expired) {
                let hint = "Google 登录已失效，请在认证中心重新登录。".to_string();
                quota.error = Some(hint.clone());
                quota.credential_message = Some(hint);
            }
            Ok(quota)
        }
        Err(error) if error == "invalid_grant" => Ok(SubscriptionQuota::error(
            "antigravity",
            CredentialStatus::Expired,
            "Google 登录已失效，请在认证中心重新登录。".to_string(),
        )),
        Err(error) => Err(error),
    }
}
