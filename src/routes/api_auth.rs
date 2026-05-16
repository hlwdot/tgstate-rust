use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Json, Router};
use openidconnect::core::{
    CoreAuthenticationFlow, CoreClient, CoreGenderClaim, CoreIdTokenClaims, CoreProviderMetadata,
};
use openidconnect::reqwest;
use openidconnect::{
    AdditionalClaims, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    SubjectIdentifier, UserInfoClaims,
};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::config::OidcSettings;
use crate::database;
use crate::state::AppState;

#[derive(Deserialize)]
struct LoginQuery {
    next: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

#[derive(Debug)]
struct OidcUser {
    subject: String,
    username: Option<String>,
    email: Option<String>,
    groups: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct AutheliaUserInfoClaims {
    #[serde(default)]
    groups: Vec<String>,
}

impl AdditionalClaims for AutheliaUserInfoClaims {}

fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .map_or(false, |v| v == "https")
}

fn extract_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|hv| hv.to_str().ok())
        .and_then(|cookies| {
            for part in cookies.split(';') {
                let kv = part.trim();
                if let Some((k, v)) = kv.split_once('=') {
                    if k == name {
                        return Some(v);
                    }
                }
            }
            None
        })
}

fn json_error(status: StatusCode, message: &str, code: &str) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({
            "status": "error",
            "code": code,
            "message": message,
        })),
    )
        .into_response()
}

fn redirect_error(message: &str, code: &str) -> axum::response::Response {
    let location = format!(
        "/login?error={}&code={}",
        percent_encoding::utf8_percent_encode(message, percent_encoding::NON_ALPHANUMERIC),
        percent_encoding::utf8_percent_encode(code, percent_encoding::NON_ALPHANUMERIC),
    );
    Redirect::to(&location).into_response()
}

fn oidc_settings(state: &AppState) -> Result<OidcSettings, axum::response::Response> {
    let settings = state.settings.oidc.clone();
    if settings.is_configured() {
        Ok(settings)
    } else {
        Err(json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "OIDC 未配置",
            "oidc_not_configured",
        ))
    }
}

fn forwarded_proto(headers: &HeaderMap) -> &str {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| *v == "http" || *v == "https")
        .unwrap_or("http")
}

fn request_host(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty() && !v.contains('/'))
}

fn oidc_redirect_url(headers: &HeaderMap) -> Result<String, axum::response::Response> {
    request_host(headers)
        .map(|host| format!("{}://{}/api/auth/callback", forwarded_proto(headers), host))
        .ok_or_else(|| {
            json_error(
                StatusCode::BAD_REQUEST,
                "请求缺少 Host，无法生成 OIDC callback",
                "missing_host",
            )
        })
}

type TgOidcClient = CoreClient<
    openidconnect::EndpointSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointNotSet,
    openidconnect::EndpointMaybeSet,
    openidconnect::EndpointMaybeSet,
>;

async fn oidc_client(
    settings: &OidcSettings,
    http_client: &reqwest::Client,
    redirect_url: String,
) -> Result<TgOidcClient, axum::response::Response> {
    let issuer = settings.issuer_url.as_deref().ok_or_else(|| {
        json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "OIDC issuer 未配置",
            "oidc_not_configured",
        )
    })?;
    let client_id = settings.client_id.as_deref().ok_or_else(|| {
        json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "OIDC client_id 未配置",
            "oidc_not_configured",
        )
    })?;
    let provider_metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(issuer.to_string()).map_err(|e| {
            tracing::error!("OIDC issuer URL 无效: {}", e);
            json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "OIDC issuer URL 无效",
                "oidc_invalid_config",
            )
        })?,
        http_client,
    )
    .await
    .map_err(|e| {
        tracing::error!("OIDC discovery 失败: {}", e);
        json_error(
            StatusCode::BAD_GATEWAY,
            "OIDC discovery 失败",
            "oidc_discovery_failed",
        )
    })?;

    let client_secret = settings
        .client_secret
        .as_ref()
        .map(|secret| ClientSecret::new(secret.clone()));

    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.to_string()),
        client_secret,
    )
    .set_redirect_uri(RedirectUrl::new(redirect_url).map_err(|e| {
        tracing::error!("OIDC redirect URL 无效: {}", e);
        json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "OIDC redirect URL 无效",
            "oidc_invalid_config",
        )
    })?);
    Ok(client)
}

fn user_from_id_token_claims(claims: &CoreIdTokenClaims) -> OidcUser {
    let username = claims
        .preferred_username()
        .map(|username| username.as_str())
        .or_else(|| {
            claims
                .name()
                .and_then(|name| name.get(None).map(|n| n.as_str()))
        })
        .map(ToOwned::to_owned);
    let email = claims.email().map(|email| email.as_str().to_string());

    OidcUser {
        subject: claims.subject().as_str().to_string(),
        username,
        email,
        groups: Vec::new(),
    }
}

fn merge_user_info(
    mut user: OidcUser,
    user_info: UserInfoClaims<AutheliaUserInfoClaims, CoreGenderClaim>,
) -> OidcUser {
    user.username = user
        .username
        .or_else(|| {
            user_info
                .preferred_username()
                .map(|v| v.as_str().to_string())
        })
        .or_else(|| {
            user_info
                .name()
                .and_then(|name| name.get(None).map(|v| v.as_str().to_string()))
        });
    user.email = user
        .email
        .or_else(|| user_info.email().map(|v| v.as_str().to_string()));
    user.groups = user_info.additional_claims().groups.clone();
    user
}

async fn login_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LoginQuery>,
) -> axum::response::Response {
    let settings = match oidc_settings(&state) {
        Ok(settings) => settings,
        Err(resp) => return resp,
    };
    let redirect_url = match oidc_redirect_url(&headers) {
        Ok(url) => url,
        Err(resp) => return resp,
    };
    let client = match oidc_client(&settings, &state.http_client, redirect_url).await {
        Ok(client) => client,
        Err(resp) => return resp,
    };

    let next_path = auth::normalize_path_for_redirect(query.next.as_deref());
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .set_pkce_challenge(pkce_challenge)
        .add_scope(Scope::new("profile".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("groups".into()))
        .url();

    if let Err(e) = database::insert_oidc_login_state(
        &state.db_pool,
        csrf_state.secret(),
        nonce.secret(),
        pkce_verifier.secret(),
        Some(&next_path),
        auth::OIDC_LOGIN_STATE_TTL_SECS,
    ) {
        tracing::error!("保存 OIDC 登录状态失败: {}", e);
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "服务器错误",
            "state_save_failed",
        );
    }

    Redirect::to(auth_url.as_str()).into_response()
}

async fn login_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> axum::response::Response {
    let settings = match oidc_settings(&state) {
        Ok(settings) => settings,
        Err(_) => return redirect_error("OIDC 未配置", "oidc_not_configured"),
    };

    let login_state = match database::take_oidc_login_state(&state.db_pool, &query.state) {
        Ok(Some(login_state)) => login_state,
        Ok(None) => return redirect_error("OIDC 登录状态已失效", "oidc_state_invalid"),
        Err(e) => {
            tracing::error!("读取 OIDC 登录状态失败: {}", e);
            return redirect_error("服务器错误", "state_read_failed");
        }
    };

    let redirect_url = match oidc_redirect_url(&headers) {
        Ok(url) => url,
        Err(_) => return redirect_error("请求缺少 Host", "missing_host"),
    };
    let client = match oidc_client(&settings, &state.http_client, redirect_url).await {
        Ok(client) => client,
        Err(_) => return redirect_error("OIDC 配置不可用", "oidc_client_failed"),
    };

    let token_request = match client.exchange_code(AuthorizationCode::new(query.code)) {
        Ok(request) => request,
        Err(e) => {
            tracing::error!("OIDC token endpoint 未配置: {}", e);
            return redirect_error("OIDC token endpoint 未配置", "oidc_token_endpoint_missing");
        }
    };

    let token_response = match token_request
        .set_pkce_verifier(PkceCodeVerifier::new(login_state.pkce_verifier))
        .request_async(&state.http_client)
        .await
    {
        Ok(token_response) => token_response,
        Err(e) => {
            tracing::warn!("OIDC token exchange 失败: {}", e);
            return redirect_error("OIDC 换取令牌失败", "oidc_token_exchange_failed");
        }
    };

    let id_token = match token_response.extra_fields().id_token() {
        Some(id_token) => id_token,
        None => return redirect_error("OIDC 响应缺少 ID Token", "oidc_missing_id_token"),
    };
    let nonce = Nonce::new(login_state.nonce);
    let claims = match id_token.claims(&client.id_token_verifier(), &nonce) {
        Ok(claims) => claims,
        Err(e) => {
            tracing::warn!("OIDC claims 解析失败: {}", e);
            return redirect_error("OIDC 登录验证失败", "oidc_claims_invalid");
        }
    };

    let user = user_from_id_token_claims(claims);
    let user_info_request = match client.user_info(
        token_response.access_token().clone(),
        Some(SubjectIdentifier::new(user.subject.clone())),
    ) {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!("OIDC UserInfo endpoint 不可用: {}", e);
            return redirect_error("OIDC UserInfo endpoint 不可用", "oidc_userinfo_unavailable");
        }
    };
    let user_info = match user_info_request
        .request_async::<AutheliaUserInfoClaims, _, CoreGenderClaim>(&state.http_client)
        .await
    {
        Ok(user_info) => user_info,
        Err(e) => {
            tracing::warn!("OIDC UserInfo 请求失败: {}", e);
            return redirect_error("OIDC UserInfo 请求失败", "oidc_userinfo_failed");
        }
    };
    let user = merge_user_info(user, user_info);

    let session_token = auth::generate_session_token();
    if let Err(e) = database::insert_auth_session(
        &state.db_pool,
        &session_token,
        &user.subject,
        user.username.as_deref(),
        user.email.as_deref(),
        auth::session_max_age_secs() as i64,
    ) {
        tracing::error!("保存 OIDC 会话失败: {}", e);
        return redirect_error("服务器错误", "session_save_failed");
    }

    tracing::info!(
        "OIDC 登录成功: subject={}, username={:?}, email={:?}",
        user.subject,
        user.username,
        user.email
    );

    let next_path = auth::normalize_path_for_redirect(login_state.next_path.as_deref());
    (
        [(
            axum::http::header::SET_COOKIE,
            auth::build_cookie(&session_token, is_https(&headers)),
        )],
        Redirect::to(&next_path),
    )
        .into_response()
}

async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if let Some(cookie) = extract_cookie(&headers, auth::COOKIE_NAME) {
        if let Err(e) = database::delete_auth_session(&state.db_pool, cookie) {
            tracing::warn!("删除会话失败: {}", e);
        }
    }

    (
        [(axum::http::header::SET_COOKIE, auth::build_clear_cookie())],
        Json(serde_json::json!({
            "status": "ok",
            "message": "已退出登录"
        })),
    )
        .into_response()
}

async fn session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    let session = extract_cookie(&headers, auth::COOKIE_NAME).and_then(|token| {
        database::get_auth_session(&state.db_pool, token)
            .ok()
            .flatten()
    });

    match session {
        Some(session) => Json(serde_json::json!({
            "status": "ok",
            "authenticated": true,
            "user": {
                "subject": session.subject,
                "username": session.username,
                "email": session.email,
            }
        }))
        .into_response(),
        None => Json(serde_json::json!({
            "status": "ok",
            "authenticated": false
        }))
        .into_response(),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/login", get(login_start))
        .route("/api/auth/callback", get(login_callback))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/session", get(session))
}
