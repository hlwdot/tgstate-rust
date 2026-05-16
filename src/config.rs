use std::collections::HashMap;

use crate::database::{self, DbPool};

pub type AppSettingsMap = HashMap<String, Option<String>>;

#[derive(Debug, Clone)]
pub struct Settings {
    pub bot_token: Option<String>,
    pub channel_name: Option<String>,
    pub picgo_api_key: Option<String>,
    pub base_url: String,
    pub _mode: String,
    pub _file_route: String,
    pub data_dir: String,
    pub oidc: OidcSettings,
}

#[derive(Debug, Clone)]
pub struct OidcSettings {
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

impl Settings {
    pub fn from_env() -> Self {
        Self {
            bot_token: std::env::var("BOT_TOKEN").ok().filter(|s| !s.is_empty()),
            channel_name: std::env::var("CHANNEL_NAME").ok().filter(|s| !s.is_empty()),
            picgo_api_key: std::env::var("PICGO_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            base_url: std::env::var("BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".into()),
            _mode: std::env::var("MODE").unwrap_or_else(|_| "p".into()),
            _file_route: std::env::var("FILE_ROUTE").unwrap_or_else(|_| "/d/".into()),
            data_dir: std::env::var("DATA_DIR").unwrap_or_else(|_| "app/data".into()),
            oidc: OidcSettings::from_env(),
        }
    }
}

impl OidcSettings {
    pub fn from_env() -> Self {
        Self {
            issuer_url: read_non_empty_env("OIDC_ISSUER_URL")
                .or_else(|| read_non_empty_env("AUTHELIA_ISSUER_URL")),
            client_id: read_non_empty_env("OIDC_CLIENT_ID")
                .or_else(|| read_non_empty_env("AUTHELIA_CLIENT_ID")),
            client_secret: read_non_empty_env("OIDC_CLIENT_SECRET")
                .or_else(|| read_non_empty_env("AUTHELIA_CLIENT_SECRET")),
        }
    }

    pub fn is_configured(&self) -> bool {
        self.issuer_url
            .as_deref()
            .map_or(false, |v| !v.trim().is_empty())
            && self
                .client_id
                .as_deref()
                .map_or(false, |v| !v.trim().is_empty())
            && self
                .client_secret
                .as_deref()
                .map_or(false, |v| !v.trim().is_empty())
    }
}

fn read_non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Merge DB settings over env settings
pub fn get_app_settings(settings: &Settings, pool: &DbPool) -> AppSettingsMap {
    let mut result = HashMap::new();
    result.insert("BOT_TOKEN".into(), settings.bot_token.clone());
    result.insert("CHANNEL_NAME".into(), settings.channel_name.clone());
    result.insert("PICGO_API_KEY".into(), settings.picgo_api_key.clone());
    result.insert("BASE_URL".into(), Some(settings.base_url.clone()));

    if let Ok(db_settings) = database::get_app_settings_from_db(pool) {
        for (key, val) in db_settings {
            if let Some(v) = &val {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    result.insert(key, Some(v));
                }
            }
        }
    }

    result
}

pub fn is_bot_ready(app_settings: &AppSettingsMap) -> bool {
    let token = app_settings
        .get("BOT_TOKEN")
        .and_then(|v| v.as_deref())
        .unwrap_or("")
        .trim();
    let channel = app_settings
        .get("CHANNEL_NAME")
        .and_then(|v| v.as_deref())
        .unwrap_or("")
        .trim();
    !token.is_empty() && !channel.is_empty()
}
