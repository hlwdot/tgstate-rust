use std::time::Duration;

use crate::constants;
use crate::database::{self, DbPool};
use crate::events::{build_file_event, BroadcastEventBus};
use crate::telegram::service::{sanitize_bot_token_in_text, TelegramService};
use crate::telegram::types::*;

pub async fn run_bot_polling(
    bot_token: String,
    channel_name: String,
    db_pool: DbPool,
    event_bus: BroadcastEventBus,
    base_url: String,
    http_client: reqwest::Client,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    // Reuse the shared `AppState::http_client` instead of constructing a new
    // one on every bot restart. Apart from avoiding per-restart connection
    // pool churn, this also ensures Telegram requests issued from the bot
    // polling loop honour the same timeouts / TLS config as every other
    // Telegram call in the process.
    let client = http_client;

    let tg_service = TelegramService::new(bot_token.clone(), channel_name.clone(), client.clone());

    let mut offset: i64 = 0;

    // Drop pending updates (equivalent to drop_pending_updates=True in python-telegram-bot)
    match get_updates(&client, &bot_token, -1, 0).await {
        Ok(updates) => {
            if let Some(last) = updates.last() {
                offset = last.update_id + 1;
                tracing::info!("跳过 {} 个待处理更新", updates.len());
            }
        }
        Err(e) => {
            tracing::warn!("清除待处理更新失败: {}", e);
        }
    }

    tracing::info!("Bot 轮询已启动");

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                tracing::info!("Bot 轮询收到关闭信号");
                break;
            }
            result = get_updates(&client, &bot_token, offset, constants::BOT_POLL_TIMEOUT_SECS as i64) => {
                match result {
                    Ok(updates) => {
                        for update in updates {
                            offset = update.update_id + 1;
                            process_update(
                                &update,
                                &tg_service,
                                &channel_name,
                                &db_pool,
                                &event_bus,
                                &base_url,
                            ).await;
                        }
                    }
                    Err(e) => {
                        tracing::error!("getUpdates 失败: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
}

async fn get_updates(
    client: &reqwest::Client,
    bot_token: &str,
    offset: i64,
    timeout: i64,
) -> Result<Vec<Update>, String> {
    let url = format!("https://api.telegram.org/bot{}/getUpdates", bot_token);
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "offset": offset,
            "timeout": timeout,
            "allowed_updates": ["message", "channel_post"]
        }))
        .timeout(Duration::from_secs(timeout as u64 + 10))
        .send()
        .await
        .map_err(|e| {
            format!(
                "Request error: {}",
                sanitize_bot_token_in_text(&e.to_string(), bot_token)
            )
        })?;

    let data: TelegramResponse<Vec<Update>> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {}", e))?;

    if data.ok {
        Ok(data.result.unwrap_or_default())
    } else {
        Err(format!(
            "getUpdates error: {}",
            data.description.unwrap_or_default()
        ))
    }
}

async fn process_update(
    update: &Update,
    tg_service: &TelegramService,
    channel_name: &str,
    db_pool: &DbPool,
    event_bus: &BroadcastEventBus,
    base_url: &str,
) {
    // Handle new file (message or channel_post)
    let message = update.message.as_ref().or(update.channel_post.as_ref());
    if let Some(msg) = message {
        if msg.document.is_some() || msg.photo.is_some() {
            handle_new_file(msg, channel_name, db_pool, event_bus).await;
        }
        // Handle "get" reply
        if let Some(text) = &msg.text {
            if text.trim().to_lowercase() == "get" && msg.reply_to_message.is_some() {
                handle_get_reply(msg, channel_name, tg_service, base_url).await;
            }
        }
    }

    // Telegram Bot API getUpdates does not provide ordinary channel message
    // deletion events. Deletes initiated from this app are handled by the API
    // routes that perform them; external channel deletes cannot be inferred
    // reliably from edited_* updates.
}

fn message_from_configured_channel(message: &Message, channel_name: &str) -> bool {
    let chat = &message.chat;
    if channel_name.starts_with('@') {
        chat.username
            .as_deref()
            .map_or(false, |u| u == channel_name.trim_start_matches('@'))
    } else {
        chat.id.to_string() == channel_name
    }
}

async fn handle_new_file(
    message: &Message,
    channel_name: &str,
    db_pool: &DbPool,
    event_bus: &BroadcastEventBus,
) {
    // Check source
    if !message_from_configured_channel(message, channel_name) {
        return;
    }

    // Extract file info
    let (file_id, file_name, file_size) = if let Some(doc) = &message.document {
        (
            doc.file_id.clone(),
            doc.file_name
                .clone()
                .unwrap_or_else(|| format!("file_{}", message.message_id)),
            doc.file_size.unwrap_or(0),
        )
    } else if let Some(photos) = &message.photo {
        if let Some(photo) = photos.last() {
            (
                photo.file_id.clone(),
                format!("photo_{}.jpg", message.message_id),
                photo.file_size.unwrap_or(0),
            )
        } else {
            return;
        }
    } else {
        return;
    };

    // Skip large files and manifests. Anything larger than the per-chunk
    // Telegram limit (`TELEGRAM_CHUNK_SIZE`) has to have been uploaded via
    // the multi-part / manifest flow, which the bot-side ingestion path
    // does not know how to reconstruct.
    if should_skip_file_sync(&file_name, file_size) {
        return;
    }

    let composite_id = format!("{}:{}", message.message_id, file_id);

    match database::add_file_metadata(db_pool, &file_name, &composite_id, file_size) {
        Ok(short_id) => {
            let upload_date = message
                .date
                .map(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();

            let event = build_file_event(
                "add",
                &composite_id,
                Some(&file_name),
                Some(file_size),
                Some(&upload_date),
                Some(&short_id),
            );
            event_bus.publish(serde_json::to_string(&event).unwrap_or_default());
        }
        Err(e) => {
            tracing::error!("添加文件元数据失败: {}", e);
        }
    }
}

fn should_skip_file_sync(file_name: &str, file_size: i64) -> bool {
    file_size as usize >= constants::TELEGRAM_CHUNK_SIZE
        || file_name.ends_with(".manifest")
        || file_name.contains(constants::TELEGRAM_CHUNK_FILENAME_MARKER)
}

async fn handle_get_reply(
    message: &Message,
    channel_name: &str,
    tg_service: &TelegramService,
    base_url: &str,
) {
    let replied = match &message.reply_to_message {
        Some(r) => r,
        None => return,
    };

    if !message_from_configured_channel(message, channel_name)
        || !message_from_configured_channel(replied, channel_name)
    {
        return;
    }

    let (file_id, file_name) = if let Some(doc) = &replied.document {
        (
            doc.file_id.clone(),
            doc.file_name
                .clone()
                .unwrap_or_else(|| format!("file_{}", replied.message_id)),
        )
    } else if let Some(photos) = &replied.photo {
        if let Some(photo) = photos.last() {
            (
                photo.file_id.clone(),
                format!("photo_{}.jpg", replied.message_id),
            )
        } else {
            let _ = send_message(
                &tg_service.client,
                &tg_service.bot_token,
                message.chat.id,
                "请回复到一个文件/图片消息",
            )
            .await;
            return;
        }
    } else {
        let _ = send_message(
            &tg_service.client,
            &tg_service.bot_token,
            message.chat.id,
            "请回复到一个文件/图片消息",
        )
        .await;
        return;
    };

    let composite_id = format!("{}:{}", replied.message_id, file_id);
    let mut final_filename = file_name.clone();

    // Check if manifest
    if file_name.ends_with(".manifest") {
        match tg_service
            .try_get_manifest_original_filename(&file_id)
            .await
        {
            Ok(name) => final_filename = name,
            Err(e) => {
                let _ = send_message(
                    &tg_service.client,
                    &tg_service.bot_token,
                    message.chat.id,
                    &format!("错误：解析清单文件失败：{}", e),
                )
                .await;
                return;
            }
        }
    }

    let encoded =
        percent_encoding::utf8_percent_encode(&final_filename, percent_encoding::NON_ALPHANUMERIC);
    let file_path = format!("/d/{}/{}", composite_id, encoded);

    let text = if !base_url.is_empty() {
        let base = base_url.trim_end_matches('/');
        format!(
            "这是 '{}' 的下载链接:\n{}{}",
            final_filename, base, file_path
        )
    } else {
        format!(
            "这是 '{}' 的下载路径 (请自行拼接域名):\n`{}`",
            final_filename, file_path
        )
    };

    let _ = send_message(
        &tg_service.client,
        &tg_service.bot_token,
        message.chat.id,
        &text,
    )
    .await;
}

async fn send_message(
    client: &reqwest::Client,
    bot_token: &str,
    chat_id: i64,
    text: &str,
) -> Result<(), String> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
    client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text
        }))
        .send()
        .await
        .map_err(|e| sanitize_bot_token_in_text(&e.to_string(), bot_token))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_skip_file_sync;
    use crate::constants;

    #[test]
    fn bot_sync_skips_internal_chunk_files() {
        assert!(should_skip_file_sync("video.mp4.tgstate-part3", 1024));
    }

    #[test]
    fn bot_sync_keeps_regular_small_part_named_files() {
        assert!(!should_skip_file_sync("archive.part3", 1024));
    }

    #[test]
    fn bot_sync_skips_large_files_and_manifests() {
        assert!(should_skip_file_sync(
            "large.bin",
            constants::TELEGRAM_CHUNK_SIZE as i64
        ));
        assert!(should_skip_file_sync("large.bin.manifest", 512));
    }
}
