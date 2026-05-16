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

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("Failed to parse upload request: {}", e);
                return Err(http_error(
                    axum::http::StatusCode::BAD_REQUEST,
                    "Malformed upload request",
                    "multipart_error",
                ));
            }
        };

        let name = field.name().unwrap_or("").to_string();
        if name != "file" {
            continue;
        }

        let raw_filename = field.file_name().unwrap_or("upload").to_string();
        let filename = sanitize_filename(&raw_filename);

        // Stream the file in chunks to Telegram.
        upload_result =
            Some(stream_upload_to_telegram(&tg_service, field, &filename, &state.db_pool).await);
        break;
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
    let mut uploaded_message_ids: Vec<i64> = Vec::new();
    let mut first_message_id: Option<i64> = None;
    let mut chunk_num: u32 = 0;

    // Read field data incrementally
    loop {
        let bytes = match field.chunk().await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => break,
            Err(e) => {
                cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
                return Err(format!("Failed to read upload data: {}", e));
            }
        };

        buffer.extend_from_slice(&bytes);
        total_size += bytes.len();

        // When buffer reaches chunk size, send it
        while buffer.len() >= chunk_size {
            chunk_num += 1;
            let chunk_data = buffer.split_to(chunk_size).freeze().to_vec();
            let chunk_name = chunk_filename(filename, chunk_num);

            let message = send_document_with_cleanup(
                tg_service,
                &mut uploaded_message_ids,
                chunk_data,
                &chunk_name,
                first_message_id,
            )
            .await?;

            if first_message_id.is_none() {
                first_message_id = Some(message.message_id);
            }

            let doc = match message.document {
                Some(doc) => doc,
                None => {
                    cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
                    return Err("No document in chunk response".into());
                }
            };
            chunk_ids.push(format!("{}:{}", message.message_id, doc.file_id));
        }
    }

    // Handle remaining data in buffer
    if buffer.is_empty() && chunk_ids.is_empty() {
        return Err("文件为空".into());
    }

    if chunk_ids.is_empty() {
        // Small file: single upload (no chunks were sent yet)
        tracing::info!(
            "Uploading file directly: {} ({} bytes)",
            filename,
            total_size
        );
        let data = buffer.freeze().to_vec();
        let message =
            send_document_with_cleanup(tg_service, &mut uploaded_message_ids, data, filename, None)
                .await?;

        let doc = match message.document {
            Some(doc) => doc,
            None => {
                cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
                return Err("No document in response".into());
            }
        };
        let composite_id = format!("{}:{}", message.message_id, doc.file_id);

        let short_id = match database::add_file_metadata(
            db_pool,
            filename,
            &composite_id,
            total_size as i64,
        ) {
            Ok(short_id) => short_id,
            Err(e) => {
                cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
                return Err(e.to_string());
            }
        };
        return Ok(short_id);
    }

    // Send remaining buffer as last chunk
    if !buffer.is_empty() {
        chunk_num += 1;
        let chunk_data = buffer.freeze().to_vec();
        let chunk_name = chunk_filename(filename, chunk_num);

        let message = send_document_with_cleanup(
            tg_service,
            &mut uploaded_message_ids,
            chunk_data,
            &chunk_name,
            first_message_id,
        )
        .await?;

        let doc = match message.document {
            Some(doc) => doc,
            None => {
                cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
                return Err("No document in last chunk response".into());
            }
        };
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
    let message = send_document_with_cleanup(
        tg_service,
        &mut uploaded_message_ids,
        manifest.into_bytes(),
        &manifest_name,
        first_message_id,
    )
    .await?;

    let doc = match message.document {
        Some(doc) => doc,
        None => {
            cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
            return Err("No document in manifest response".into());
        }
    };
    let manifest_composite = format!("{}:{}", message.message_id, doc.file_id);

    let short_id = match database::add_file_metadata(
        db_pool,
        filename,
        &manifest_composite,
        total_size as i64,
    ) {
        Ok(short_id) => short_id,
        Err(e) => {
            cleanup_uploaded_messages(tg_service, &uploaded_message_ids).await;
            return Err(e.to_string());
        }
    };
    Ok(short_id)
}

async fn send_document_with_cleanup(
    tg_service: &TelegramService,
    uploaded_message_ids: &mut Vec<i64>,
    data: Vec<u8>,
    filename: &str,
    reply_to: Option<i64>,
) -> Result<crate::telegram::types::Message, String> {
    match tg_service.send_document_raw(data, filename, reply_to).await {
        Ok(message) => {
            uploaded_message_ids.push(message.message_id);
            Ok(message)
        }
        Err(e) => {
            cleanup_uploaded_messages(tg_service, uploaded_message_ids).await;
            Err(e)
        }
    }
}

fn chunk_filename(filename: &str, chunk_num: u32) -> String {
    format!(
        "{}{}{}",
        filename,
        constants::TELEGRAM_CHUNK_FILENAME_MARKER,
        chunk_num
    )
}

async fn cleanup_uploaded_messages(tg_service: &TelegramService, message_ids: &[i64]) {
    for message_id in message_ids.iter().rev() {
        let (ok, reason) = tg_service.delete_message(*message_id).await;
        if !ok {
            tracing::warn!(
                "Failed to clean up partial upload message: message_id={}, reason={}",
                message_id,
                reason
            );
        }
    }
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

    fn malformed_multipart_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/upload")
            .header(
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=X-BOUNDARY",
            )
            .body(Body::from("--wrong-boundary\r\nbroken"))
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

    #[tokio::test]
    async fn upload_route_rejects_malformed_multipart() {
        let state = test_state();
        let app = Router::new()
            .merge(super::router())
            .with_state(state.clone());
        let response = app.oneshot(malformed_multipart_request()).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let files = database::get_all_files(&state.db_pool).unwrap();
        assert!(files.is_empty(), "unexpected files persisted: {:?}", files);
    }
}
