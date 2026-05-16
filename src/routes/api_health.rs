use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use std::sync::Arc;

use crate::state::AppState;

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok"
    }))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/health", get(health))
}
