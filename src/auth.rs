pub const COOKIE_NAME: &str = "tgstate_session";
pub const OIDC_LOGIN_STATE_TTL_SECS: i64 = 10 * 60;

use std::sync::OnceLock;

use rand::RngCore;

use crate::constants;

/// Generate a cryptographically random session token (32 bytes, hex-encoded -> 64 chars).
/// The opaque token is stored server-side in `auth_sessions` and set as the
/// browser session cookie after a successful OIDC callback.
pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn parse_truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Read and cache the `COOKIE_SECURE` env override. When set to a truthy value
/// (`1`/`true`/`yes`/`on`), session cookies are always marked `Secure` regardless
/// of request detection.
fn cookie_secure_override() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("COOKIE_SECURE")
            .map(|v| parse_truthy(&v))
            .unwrap_or(false)
    })
}

/// Read and cache the `SESSION_MAX_AGE_SECS` env override; fall back to the constant.
pub fn session_max_age_secs() -> u32 {
    static CACHED: OnceLock<u32> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("SESSION_MAX_AGE_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(constants::SESSION_MAX_AGE_SECS)
    })
}

/// Build a session cookie string with security flags.
///
/// `is_https` is honored when true; the `COOKIE_SECURE` env var can force `Secure`
/// regardless. `SESSION_MAX_AGE_SECS` env controls the Max-Age (defaulting to
/// `constants::SESSION_MAX_AGE_SECS`).
pub fn build_cookie(value: &str, is_https: bool) -> String {
    let secure = if is_https || cookie_secure_override() {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        COOKIE_NAME,
        value,
        session_max_age_secs(),
        secure
    )
}

/// Build a cookie that clears the session.
pub fn build_clear_cookie() -> String {
    format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        COOKIE_NAME
    )
}

pub fn normalize_path_for_redirect(input: Option<&str>) -> String {
    let path = input.unwrap_or("/").trim();
    if !path.starts_with('/') || path.starts_with("//") || path.contains("://") {
        return "/".to_string();
    }

    let public_or_auth_path = path == "/login"
        || path == "/pwd"
        || path == "/logout"
        || path.starts_with("/api/")
        || path.starts_with("/static/")
        || path.starts_with("/assets/");
    if public_or_auth_path {
        "/".to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{generate_session_token, normalize_path_for_redirect};

    #[test]
    fn generate_session_token_is_64_hex_chars() {
        let t = generate_session_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t, generate_session_token());
    }

    #[test]
    fn redirect_path_rejects_external_targets() {
        assert_eq!(normalize_path_for_redirect(Some("https://evil.test")), "/");
        assert_eq!(normalize_path_for_redirect(Some("//evil.test")), "/");
        assert_eq!(normalize_path_for_redirect(Some("/settings")), "/settings");
        assert_eq!(normalize_path_for_redirect(Some("/api/files")), "/");
    }
}
