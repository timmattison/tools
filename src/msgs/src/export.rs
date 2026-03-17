use std::path::{Path, PathBuf};

use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::types::ExportResult;

/// Export a range of messages from a conversation to JSON + attachments.
///
/// # Errors
///
/// Returns `MsgsError::ExportError` on filesystem or serialization failures.
pub fn export_conversation_range(
    db: &MessageDb,
    chat_id: i64,
    start_date: i64,
    end_date: i64,
    include_attachments: bool,
    export_dir: &Path,
) -> Result<ExportResult, MsgsError> {
    // Create export directory
    std::fs::create_dir_all(export_dir)
        .map_err(|e| MsgsError::ExportError(format!("Cannot create export dir: {e}")))?;

    // Get messages in date range
    let messages = db.get_messages_in_date_range(chat_id, start_date, end_date)?;

    // Get conversation metadata
    let conversation = db.get_conversation(chat_id)?;

    let mut attachment_count: i64 = 0;

    // Copy attachments if requested
    if include_attachments {
        let att_dir = export_dir.join("attachments");
        std::fs::create_dir_all(&att_dir)
            .map_err(|e| MsgsError::ExportError(format!("Cannot create attachments dir: {e}")))?;

        let allowed_prefix = dirs::home_dir()
            .map(|h| h.join("Library/Messages/Attachments"))
            .unwrap_or_default();

        for msg in &messages {
            for att in &msg.attachments {
                if let Some(ref filename) = att.filename {
                    let source = expand_tilde(filename);
                    // Path traversal prevention: only copy from Messages/Attachments
                    if !source.starts_with(&allowed_prefix) {
                        log::warn!(
                            "Attachment path outside allowed directory: {}",
                            source.display()
                        );
                        continue;
                    }
                    if source.exists() {
                        let dest_name = source
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let dest = att_dir.join(&dest_name);
                        if let Err(e) = std::fs::copy(&source, &dest) {
                            log::warn!("Cannot copy attachment {}: {e}", source.display());
                        } else {
                            attachment_count += 1;
                        }
                    } else {
                        log::warn!("Attachment file not found: {}", source.display());
                    }
                }
            }
        }
    }

    // Build export JSON
    let export_data = serde_json::json!({
        "conversation": {
            "chat_identifier": conversation.chat_identifier,
            "display_name": conversation.display_name,
            "participants": conversation.participants,
            "is_group": conversation.is_group,
        },
        "export_range": {
            "start_date": messages.first().map(|m| &m.date),
            "end_date": messages.last().map(|m| &m.date),
            "message_count": messages.len(),
        },
        "messages": messages,
    });

    let json_path = export_dir.join("messages.json");
    let json_str = serde_json::to_string_pretty(&export_data)
        .map_err(|e| MsgsError::ExportError(format!("JSON serialization failed: {e}")))?;

    std::fs::write(&json_path, json_str)
        .map_err(|e| MsgsError::ExportError(format!("Cannot write JSON: {e}")))?;

    #[expect(
        clippy::cast_possible_wrap,
        reason = "message count will not exceed i64::MAX"
    )]
    Ok(ExportResult {
        export_path: export_dir.display().to_string(),
        message_count: messages.len() as i64,
        attachment_count,
    })
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}
