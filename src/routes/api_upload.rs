use std::sync::Arc;

use axum::extract::{Multipart, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use bytes::BytesMut;

use crate::config;
use crate::constants;
use crate::database;
use crate::error::http_error;
use crate::state::AppState;
use crate::telegram::service::TelegramService;

/// Sanitize filename: extract basename, limit length, remove dangerous chars.
fn sanitize_filename(raw: &str) -> String {
    let name = std::path::Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload");
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();
    if clean.is_empty() {
        return "upload".to_string();
    }
    // UTF-8-safe byte-length cap: `clean[..255]` would panic if byte 255
    // falls inside a multibyte character (e.g. a Chinese filename).
    if clean.len() <= 255 {
        return clean;
    }
    let mut cutoff = 0;
    for (idx, _) in clean.char_indices() {
        if idx > 255 {
            break;
        }
        cutoff = idx;
    }
    clean[..cutoff].to_string()
}

async fn upload_file(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let app_settings = config::get_app_settings(&state.settings, &state.db_pool);

    let bot_token = app_settings
        .get("BOT_TOKEN")
        .and_then(|v| v.as_deref())
        .unwrap_or("");
    let channel_name = app_settings
        .get("CHANNEL_NAME")
        .and_then(|v| v.as_deref())
        .unwrap_or("");

    if bot_token.is_empty() || channel_name.is_empty() {
        return Err(http_error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "缺少 BOT_TOKEN 或 CHANNEL_NAME，无法上传",
            "cfg_missing",
        ));
    }

    // Parse multipart body - stream file chunks to Telegram
    let mut upload_result: Option<Result<String, String>> = None;

    let tg_service = TelegramService::new(
        bot_token.to_string(),
        channel_name.to_string(),
        state.http_client.clone(),
    );

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            let raw_filename = field.file_name().unwrap_or("upload").to_string();
            let filename = sanitize_filename(&raw_filename);

            // Stream the file in chunks to Telegram
            upload_result = Some(
                stream_upload_to_telegram(&tg_service, field, &filename, &state.db_pool).await,
            );
        }
    }

    let short_id = upload_result
        .ok_or_else(|| http_error(axum::http::StatusCode::BAD_REQUEST, "未提供文件", "no_file"))?
        .map_err(|e| {
            tracing::error!("文件上传失败: {}", e);
            http_error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "文件上传失败",
                "upload_failed",
            )
        })?;

    let download_path = format!("/d/{}", short_id);
    Ok(Json(serde_json::json!({
        "file_id": short_id,
        "short_id": short_id,
        "download_path": download_path,
        "path": download_path,
        "url": download_path,
    })))
}

/// Stream file upload: reads multipart field in chunks, uploads each chunk to Telegram
/// as it reaches TELEGRAM_CHUNK_SIZE. Peak memory is ~1 chunk (~20MB) instead of the full file.
async fn stream_upload_to_telegram(
    tg_service: &TelegramService,
    mut field: axum::extract::multipart::Field<'_>,
    filename: &str,
    db_pool: &database::DbPool,
) -> Result<String, String> {
    let chunk_size = constants::TELEGRAM_CHUNK_SIZE;
    let mut buffer = BytesMut::with_capacity(chunk_size);
    let mut total_size: usize = 0;
    let mut chunk_ids: Vec<String> = Vec::new();
    let mut first_message_id: Option<i64> = None;
    let mut chunk_num: u32 = 0;

    // Read field data incrementally
    while let Ok(Some(bytes)) = field.chunk().await {
        buffer.extend_from_slice(&bytes);
        total_size += bytes.len();

        // When buffer reaches chunk size, send it
        while buffer.len() >= chunk_size {
            chunk_num += 1;
            let chunk_data = buffer.split_to(chunk_size).freeze().to_vec();
            let chunk_name = format!("{}.part{}", filename, chunk_num);

            let message = tg_service
                .send_document_raw(chunk_data, &chunk_name, first_message_id)
                .await?;

            if first_message_id.is_none() {
                first_message_id = Some(message.message_id);
            }

            let doc = message.document.ok_or("No document in chunk response")?;
            chunk_ids.push(format!("{}:{}", message.message_id, doc.file_id));
        }
    }

    // Handle remaining data in buffer
    if buffer.is_empty() && chunk_ids.is_empty() {
        return Err("文件为空".into());
    }

    if chunk_ids.is_empty() {
        // Small file: single upload (no chunks were sent yet)
        tracing::info!("直接上传文件: {} ({}字节)", filename, total_size);
        let data = buffer.freeze().to_vec();
        let message = tg_service.send_document_raw(data, filename, None).await?;

        let doc = message.document.ok_or("No document in response")?;
        let composite_id = format!("{}:{}", message.message_id, doc.file_id);

        let short_id =
            database::add_file_metadata(db_pool, filename, &composite_id, total_size as i64)
                .map_err(|e| e.to_string())?;
        return Ok(short_id);
    }

    // Send remaining buffer as last chunk
    if !buffer.is_empty() {
        chunk_num += 1;
        let chunk_data = buffer.freeze().to_vec();
        let chunk_name = format!("{}.part{}", filename, chunk_num);

        let message = tg_service
            .send_document_raw(chunk_data, &chunk_name, first_message_id)
            .await?;

        let doc = message
            .document
            .ok_or("No document in last chunk response")?;
        chunk_ids.push(format!("{}:{}", message.message_id, doc.file_id));
    }

    // Multi-chunk: create and upload manifest
    tracing::info!(
        "分块上传完成: {} ({}MB, {} 块)",
        filename,
        total_size / (1024 * 1024),
        chunk_ids.len()
    );

    let mut manifest = String::from("tgstate-blob\n");
    manifest.push_str(filename);
    manifest.push('\n');
    for cid in &chunk_ids {
        manifest.push_str(cid);
        manifest.push('\n');
    }

    let manifest_name = format!("{}.manifest", filename);
    let message = tg_service
        .send_document_raw(manifest.into_bytes(), &manifest_name, first_message_id)
        .await?;

    let doc = message.document.ok_or("No document in manifest response")?;
    let manifest_composite = format!("{}:{}", message.message_id, doc.file_id);

    let short_id =
        database::add_file_metadata(db_pool, filename, &manifest_composite, total_size as i64)
            .map_err(|e| e.to_string())?;
    Ok(short_id)
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/upload", post(upload_file))
}

#[cfg(test)]
mod tests {
    use crate::config::{OidcSettings, Settings};
    use crate::database;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use axum::Router;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::util::ServiceExt;

    fn test_state() -> Arc<AppState> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let data_dir = std::env::temp_dir()
            .join(format!("tgstate-upload-test-{}", unique))
            .to_string_lossy()
            .to_string();

        let settings = Settings {
            bot_token: Some("123456:test-token".into()),
            channel_name: Some("@test_channel".into()),
            base_url: "http://127.0.0.1:8000".into(),
            _mode: "p".into(),
            _file_route: "/d/".into(),
            data_dir: data_dir.clone(),
            oidc: OidcSettings {
                issuer_url: Some("https://auth.example.com".into()),
                client_id: Some("tgstate".into()),
                client_secret: Some("secret".into()),
            },
        };

        let db_pool = database::init_db(&data_dir);
        let tera = tera::Tera::default();
        let http_client = reqwest::Client::new();
        let app_settings = crate::config::get_app_settings(&settings, &db_pool);
        Arc::new(AppState::new(
            settings,
            tera,
            http_client,
            db_pool,
            app_settings,
            true,
        ))
    }

    fn multipart_request_without_file() -> Request<Body> {
        let boundary = "X-BOUNDARY";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nignored\r\n--{b}--\r\n",
            b = boundary
        );

        Request::builder()
            .method("POST")
            .uri("/api/upload")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={}", boundary),
            )
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn upload_route_rejects_missing_file_field() {
        let state = test_state();
        let app = Router::new()
            .merge(super::router())
            .with_state(state.clone());
        let response = app.oneshot(multipart_request_without_file()).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let files = database::get_all_files(&state.db_pool).unwrap();
        assert!(files.is_empty(), "unexpected files persisted: {:?}", files);
    }
}
