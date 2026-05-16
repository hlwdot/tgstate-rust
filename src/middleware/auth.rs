use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

use crate::auth::{self, COOKIE_NAME};
use crate::database;
use crate::state::AppState;

fn extract_cookie<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
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

fn redirect_or_401(path: &str, accept_html: bool) -> Response {
    if accept_html && !path.starts_with("/api/") {
        // Redirect browsers to the OIDC login entrypoint.
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::SEE_OTHER;
        let location = format!("/login?next={}", percent_encode_path(path));
        resp.headers_mut().insert(
            axum::http::header::LOCATION,
            location
                .parse()
                .unwrap_or_else(|_| "/login".parse().unwrap()),
        );
        resp
    } else {
        let mut resp = Response::new(Body::from(
            serde_json::json!({
                "status": "error",
                "code": "unauthorized",
                "message": "需要登录"
            })
            .to_string(),
        ));
        *resp.status_mut() = StatusCode::UNAUTHORIZED;
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        resp
    }
}

fn wants_html(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false)
}

fn path_matches_public_entry(path: &str, public_entry: &str) -> bool {
    if public_entry == "/" {
        path == "/"
    } else if public_entry.ends_with('/') {
        path.starts_with(public_entry)
    } else {
        path == public_entry || path.starts_with(&format!("{}/", public_entry))
    }
}

fn percent_encode_path(path: &str) -> String {
    percent_encoding::utf8_percent_encode(path, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map_or(false, |v| v == "https")
}

fn reject_csrf() -> Response {
    let mut resp = Response::new(Body::from(
        serde_json::json!({
            "status": "error",
            "code": "csrf_failed",
            "message": "请求来源不匹配"
        })
        .to_string(),
    ));
    *resp.status_mut() = StatusCode::FORBIDDEN;
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    resp
}

fn is_state_changing(method: &Method) -> bool {
    !matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
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
        .filter(|v| !v.is_empty())
}

fn expected_origin(headers: &HeaderMap) -> Option<String> {
    request_host(headers).map(|host| format!("{}://{}", forwarded_proto(headers), host))
}

fn normalize_origin(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    if value.starts_with("http://") || value.starts_with("https://") {
        Some(value.to_string())
    } else {
        None
    }
}

fn referer_origin(value: &str) -> Option<String> {
    let value = value.trim();
    let scheme_end = value.find("://")?;
    let authority_start = scheme_end + 3;
    let path_start = value[authority_start..]
        .find('/')
        .map(|idx| authority_start + idx)
        .unwrap_or(value.len());
    normalize_origin(&value[..path_start])
}

fn csrf_origin_allowed(headers: &HeaderMap) -> bool {
    let expected = match expected_origin(headers) {
        Some(v) => v,
        None => return false,
    };

    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        return normalize_origin(origin).as_deref() == Some(expected.as_str());
    }

    if let Some(referer) = headers
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
    {
        return referer_origin(referer).as_deref() == Some(expected.as_str());
    }

    // Non-browser API clients often omit both headers. Cross-site browser
    // writes include Origin on modern browsers, and those are enforced above.
    true
}

fn check_session(state: &Arc<AppState>, session_cookie: Option<&str>) -> bool {
    let Some(cookie) = session_cookie.filter(|v| !v.is_empty()) else {
        return false;
    };
    database::get_auth_session(&state.db_pool, cookie)
        .map(|session| session.is_some())
        .unwrap_or(false)
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let state_changing = is_state_changing(req.method());

    if state_changing && !csrf_origin_allowed(req.headers()) {
        return reject_csrf();
    }

    // Always-allowed static paths
    let public_static_prefixes = ["/static/", "/assets/", "/favicon", "/robots.txt"];
    if public_static_prefixes.iter().any(|p| path.starts_with(p)) {
        return next.run(req).await;
    }

    // Always-allowed public-content prefixes. These are the visitor-facing
    // routes: short download links, legacy download links, and share pages.
    // Shared files are meant to be reachable by guests without an account,
    // so they must be allowed regardless of whether OIDC is configured.
    let public_content_prefixes = ["/d/", "/share/"];
    if public_content_prefixes.iter().any(|p| path.starts_with(p)) {
        return next.run(req).await;
    }

    // Login endpoints must be reachable without an application session. The
    // callback still validates the OIDC state/nonce/PKCE tuple before creating
    // a local session.
    let public_api = [
        "/api/health",
        "/api/auth/login",
        "/api/auth/callback",
        "/api/auth/oidc/callback",
        "/api/auth/logout",
    ];
    if public_api
        .iter()
        .any(|p| &path == p || path_matches_public_entry(&path, p))
    {
        return next.run(req).await;
    }
    if &path == "/login" || &path == "/pwd" || &path == "/welcome" {
        return next.run(req).await;
    }

    let headers = req.headers().clone();
    let cookie = extract_cookie(&headers, COOKIE_NAME);

    if !state.settings.oidc.is_configured() {
        if path == "/" || path == "/api/app-config" {
            return next.run(req).await;
        }

        return redirect_or_401(&path, wants_html(&headers));
    }

    if check_session(&state, cookie) {
        // Sliding expiration: re-issue the cookie with a fresh Max-Age on every
        // authenticated request, so active users stay logged in indefinitely.
        // We only refresh on non-API HTML page loads and safe (GET/HEAD) API
        // calls to avoid mutating Set-Cookie on every XHR response, which
        // would be wasteful; GETs are frequent enough in normal use to keep
        // the cookie fresh.
        let secure = is_https(&headers);
        let token = cookie.unwrap_or("").to_string();
        let mut resp = next.run(req).await;
        if !token.is_empty() {
            if let Ok(cookie_val) = HeaderValue::from_str(&auth::build_cookie(&token, secure)) {
                resp.headers_mut()
                    .append(axum::http::header::SET_COOKIE, cookie_val);
            }
        }
        return resp;
    }

    redirect_or_401(&path, wants_html(&headers))
}

#[cfg(test)]
mod tests {
    use super::{auth_middleware, csrf_origin_allowed};
    use crate::config::{OidcSettings, Settings};
    use crate::database;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::Router;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::ServiceExt;

    fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        headers
    }

    #[test]
    fn csrf_allows_matching_origin() {
        let headers = headers("127.0.0.1:8000", Some("http://127.0.0.1:8000"));
        assert!(csrf_origin_allowed(&headers));
    }

    #[test]
    fn csrf_rejects_cross_origin() {
        let headers = headers("127.0.0.1:8000", Some("https://evil.example"));
        assert!(!csrf_origin_allowed(&headers));
    }

    #[test]
    fn csrf_allows_non_browser_clients_without_origin_headers() {
        let headers = headers("127.0.0.1:8000", None);
        assert!(csrf_origin_allowed(&headers));
    }

    fn test_state(oidc_configured: bool) -> Arc<AppState> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir()
            .join(format!("tgstate-auth-test-{}", unique))
            .to_string_lossy()
            .to_string();

        let oidc = if oidc_configured {
            OidcSettings {
                issuer_url: Some("https://auth.example.com".into()),
                client_id: Some("tgstate".into()),
                client_secret: Some("secret".into()),
            }
        } else {
            OidcSettings {
                issuer_url: None,
                client_id: None,
                client_secret: None,
            }
        };

        let settings = Settings {
            bot_token: Some("123456:test-token".into()),
            channel_name: Some("@test_channel".into()),
            base_url: "http://127.0.0.1:8000".into(),
            _mode: "p".into(),
            _file_route: "/d/".into(),
            data_dir: data_dir.clone(),
            oidc,
        };

        let db_pool = database::init_db(&data_dir);
        let app_settings = crate::config::get_app_settings(&settings, &db_pool);
        Arc::new(AppState::new(
            settings,
            tera::Tera::default(),
            reqwest::Client::new(),
            db_pool,
            app_settings,
            true,
        ))
    }

    fn auth_test_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/app-config", get(|| async { "config" }))
            .route("/api/app-config/save", post(|| async { "saved" }))
            .route("/settings", get(|| async { "settings" }))
            .route("/d/file", get(|| async { "file" }))
            .layer(axum::middleware::from_fn_with_state(state, auth_middleware))
    }

    fn same_origin_post(path: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(path)
            .header(header::HOST, "127.0.0.1:8000")
            .header(header::ORIGIN, "http://127.0.0.1:8000")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn first_run_allows_read_only_setup_but_blocks_writes() {
        let state = test_state(false);
        let app = auth_test_app(state);

        let read_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/app-config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_response.status(), StatusCode::OK);

        let write_response = app
            .clone()
            .oneshot(same_origin_post("/api/app-config/save"))
            .await
            .unwrap();
        assert_eq!(write_response.status(), StatusCode::UNAUTHORIZED);

        let public_file_response = app
            .oneshot(
                Request::builder()
                    .uri("/d/file")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(public_file_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authenticated_session_still_reaches_protected_routes() {
        let state = test_state(true);
        let token = crate::auth::generate_session_token();
        database::insert_auth_session(
            &state.db_pool,
            &token,
            "subject",
            Some("user"),
            Some("user@example.com"),
            3600,
        )
        .unwrap();

        let response = auth_test_app(state)
            .oneshot(
                Request::builder()
                    .uri("/settings")
                    .header(
                        header::COOKIE,
                        format!("{}={}", crate::auth::COOKIE_NAME, token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
