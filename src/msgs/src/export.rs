use std::path::Path;

use crate::db::MessageDb;
use crate::error::MsgsError;
use crate::types::ExportResult;
use crate::util::resolve_attachment;

/// Export a range of messages from a conversation to JSON + attachments.
/// The range runs from the start message to the end message, both included,
/// in either order.
///
/// An attachment is copied only when its path, after `..` and symlinks
/// resolve, lies inside `attachments_root`.
///
/// # Errors
///
/// Returns `MsgsError::ExportError` on filesystem or serialization failures.
pub fn export_conversation_range(
    db: &MessageDb,
    chat_id: i64,
    start_message_id: i64,
    end_message_id: i64,
    include_attachments: bool,
    export_dir: &Path,
    attachments_root: &Path,
) -> Result<ExportResult, MsgsError> {
    // Create export directory
    std::fs::create_dir_all(export_dir)
        .map_err(|e| MsgsError::ExportError(format!("Cannot create export dir: {e}")))?;

    // Get the messages from the start message to the end message
    let messages = db.get_messages_between(chat_id, start_message_id, end_message_id)?;

    // Get conversation metadata
    let conversation = db.get_conversation(chat_id)?;

    let mut attachment_count: i64 = 0;

    // Copy attachments if requested
    if include_attachments {
        let att_dir = export_dir.join("attachments");
        std::fs::create_dir_all(&att_dir)
            .map_err(|e| MsgsError::ExportError(format!("Cannot create attachments dir: {e}")))?;

        for msg in &messages {
            for att in &msg.attachments {
                if let Some(ref filename) = att.filename {
                    let Some(source) = resolve_attachment(filename, attachments_root) else {
                        log::warn!("Attachment path not allowed or not found: {filename}");
                        continue;
                    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixture::{apple_ns, ChatDbFixture, MessageSpec};
    use crate::util::expand_tilde;

    const BASE: i64 = 1_700_000_000;

    /// A root for a test that copies no attachment.
    fn no_root() -> &'static Path {
        Path::new("/nonexistent-attachments-root")
    }

    #[test]
    fn export_without_attachments_writes_messages_json() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("alice@example.com");
        let chat = fx.add_chat("chat-export", Some("Export me"), 43);
        let other = fx.add_chat("other", None, 45);
        fx.link_handle(chat, alice);
        let mut ids = std::collections::HashMap::new();
        for (text, secs, chats) in [
            ("too early", 5, vec![chat]),
            ("first", 10, vec![chat]),
            ("second", 20, vec![chat]),
            ("elsewhere", 15, vec![other]),
            ("too late", 40, vec![chat]),
        ] {
            let id = fx.add_message(MessageSpec {
                text: Some(text.to_string()),
                handle: alice,
                date: apple_ns(BASE + secs, 0),
                chats,
                ..MessageSpec::default()
            });
            ids.insert(text, id);
        }
        let db = fx.open_db();
        let export_dir = fx.dir().join("export").join("nested");

        let result = export_conversation_range(
            &db,
            chat,
            ids["first"],
            ids["second"],
            false,
            &export_dir,
            no_root(),
        )
        .expect("export");

        assert_eq!(result.message_count, 2);
        assert_eq!(result.attachment_count, 0);
        assert_eq!(result.export_path, export_dir.display().to_string());
        assert!(!export_dir.join("attachments").exists());

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(export_dir.join("messages.json")).expect("read json"),
        )
        .expect("parse json");
        assert_eq!(json["conversation"]["chat_identifier"], "chat-export");
        assert_eq!(json["conversation"]["display_name"], "Export me");
        assert_eq!(json["conversation"]["is_group"], true);
        assert_eq!(json["export_range"]["message_count"], 2);
        let texts: Vec<_> = json["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .map(|m| m["text"].as_str().expect("text"))
            .collect();
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn export_includes_both_ends_when_their_dates_have_sub_second_parts() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("alice@example.com");
        let chat = fx.add_chat("chat-subsecond", None, 45);
        let ids: Vec<i64> = [
            apple_ns(BASE + 10, 250_000_000),
            apple_ns(BASE + 20, 0),
            apple_ns(BASE + 30, 0),
            apple_ns(BASE + 40, 750_000_000),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, date)| {
            fx.add_message(MessageSpec {
                text: Some(format!("m{i}")),
                handle: alice,
                date,
                chats: vec![chat],
                ..MessageSpec::default()
            })
        })
        .collect();
        let db = fx.open_db();
        let export_dir = fx.dir().join("export-subsecond");

        export_conversation_range(&db, chat, ids[0], ids[3], false, &export_dir, no_root())
            .expect("export");

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(export_dir.join("messages.json")).expect("read json"),
        )
        .expect("parse json");
        let exported: Vec<i64> = json["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .map(|m| m["message_id"].as_i64().expect("message id"))
            .collect();
        assert_eq!(exported, ids);
    }

    #[test]
    fn export_takes_the_ids_in_either_order_and_refuses_a_foreign_id() {
        let fx = ChatDbFixture::new();
        let alice = fx.add_handle("alice@example.com");
        let chat = fx.add_chat("chat-order", None, 45);
        let other = fx.add_chat("other-order", None, 45);
        let add = |secs: i64, chats: Vec<i64>| {
            fx.add_message(MessageSpec {
                text: Some("x".to_string()),
                handle: alice,
                date: apple_ns(BASE + secs, 0),
                chats,
                ..MessageSpec::default()
            })
        };
        let first = add(10, vec![chat]);
        let middle = add(20, vec![chat]);
        let last = add(30, vec![chat]);
        let foreign = add(25, vec![other]);
        let db = fx.open_db();

        let ids_of = |name: &str, a: i64, b: i64| -> Vec<i64> {
            let dir = fx.dir().join(name);
            export_conversation_range(&db, chat, a, b, false, &dir, no_root()).expect("export");
            let json: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join("messages.json")).expect("read json"),
            )
            .expect("parse json");
            json["messages"]
                .as_array()
                .expect("messages array")
                .iter()
                .map(|m| m["message_id"].as_i64().expect("message id"))
                .collect()
        };
        assert_eq!(ids_of("forward", first, last), vec![first, middle, last]);
        assert_eq!(ids_of("reverse", last, first), vec![first, middle, last]);

        let dir = fx.dir().join("foreign");
        let error = export_conversation_range(&db, chat, first, foreign, false, &dir, no_root())
            .expect_err("foreign id");
        assert!(error.to_string().contains(&foreign.to_string()));
    }

    #[test]
    fn export_writes_a_null_date_for_a_message_with_date_zero() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("chat-undated", None, 45);
        let id = fx.add_message(MessageSpec {
            text: Some("no date".to_string()),
            date: 0,
            chats: vec![chat],
            ..MessageSpec::default()
        });
        let db = fx.open_db();
        let export_dir = fx.dir().join("export-undated");

        export_conversation_range(&db, chat, id, id, false, &export_dir, no_root())
            .expect("export");

        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(export_dir.join("messages.json")).expect("read json"),
        )
        .expect("parse json");
        assert_eq!(json["messages"][0]["date"], serde_json::Value::Null);
    }

    #[test]
    fn export_refuses_an_attachment_path_that_climbs_out_with_dot_dot() {
        let fx = ChatDbFixture::new();
        let chat = fx.add_chat("chat-traversal", None, 45);
        let id = fx.add_message(MessageSpec {
            text: Some("secret".to_string()),
            date: apple_ns(BASE + 10, 0),
            chats: vec![chat],
            ..MessageSpec::default()
        });
        let outside = fx.dir().join("outside");
        std::fs::create_dir_all(&outside).expect("outside dir");
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "top secret").expect("write secret");

        let home = dirs::home_dir().expect("home directory");
        let climbs = home.components().count() + 3 + 4;
        let traversal = format!(
            "~/Library/Messages/Attachments/{}{}",
            "../".repeat(climbs),
            secret.display().to_string().trim_start_matches('/')
        );
        // The path reaches the file when the shell resolves it.
        assert_eq!(
            std::fs::read_to_string(expand_tilde(&traversal))
                .ok()
                .as_deref(),
            Some("top secret")
        );
        fx.add_attachment(id, &traversal, Some("secret.txt"));
        let db = fx.open_db();
        let export_dir = fx.dir().join("export-traversal");
        let root = home.join("Library/Messages/Attachments");

        let result =
            export_conversation_range(&db, chat, id, id, true, &export_dir, &root).expect("export");

        assert_eq!(result.attachment_count, 0);
        let copied = std::fs::read_dir(export_dir.join("attachments"))
            .expect("attachments dir")
            .count();
        assert_eq!(copied, 0);
    }
}
