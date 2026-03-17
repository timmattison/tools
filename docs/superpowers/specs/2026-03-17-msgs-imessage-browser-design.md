# msgs — iMessage Browser & Exporter

## Overview

A Tauri desktop application for browsing, searching, and exporting iMessage conversations. Reads directly from macOS's `~/Library/Messages/chat.db` in read-only mode. Built as part of the Buffalo Tools monorepo at `src/msgs/`.

## Problem

Messages.app has poor text selection and no export capability. The iMessage SQLite database contains all the data but requires manual SQL queries to access. This app provides a GUI for browsing, searching, and exporting message ranges with attachments.

## Architecture

**Approach: Direct SQLite, read-only.** No shadow database or import step. The Rust backend opens `chat.db` read-only and queries Apple's schema directly. If search performance becomes insufficient, a hybrid FTS5 index can be added incrementally.

### Backend (Rust)

All business logic lives in Rust:

- **DB Layer**: Read-only SQLite connection to `~/Library/Messages/chat.db` via `rusqlite`. Requires Full Disk Access granted to the app.
- **attributedBody Parser**: Extracts plain text from NSKeyedArchiver/streamtyped blobs. 99.9% of messages (235k of 237k) store text in `attributedBody`, not the `text` column. The blob format is: `NSString` marker at byte offset, then 4 header bytes (`\x01\x94\x84\x01`), a type byte (`0x2b` for short strings, `0x2d` for long), a length encoding, then UTF-8 text.
- **Date Conversion**: Apple Core Data timestamps are nanoseconds since 2001-01-01. Convert via `date / 1_000_000_000 + 978_307_200` to Unix epoch.
- **Query Builder**: Constructs SQL against Apple's schema (`message`, `chat`, `handle`, `attachment`, and their join tables).
- **Export Engine**: Serializes message ranges to JSON and copies attachment files to an export folder.
- **Search Engine**: LIKE-based queries, optionally filtered to a specific chat_id.

### Frontend (Vanilla TypeScript + Vite)

Pure view layer — no application state beyond UI concerns (scroll position, selection mode, form inputs). All data fetched via Tauri commands.

### Type Safety

Rust structs generate TypeScript types via `ts-rs` or `specta`. No manual type synchronization.

### IPC

All frontend-to-backend communication via typed Tauri commands. Backend pushes updates via Tauri events where needed.

## Data Model

### Core Structs

```rust
struct Conversation {
    chat_id: i64,
    chat_identifier: String,    // phone number, email, or URN
    display_name: Option<String>, // group chat name
    is_group: bool,             // style 43 = group, 45 = 1-on-1
    participants: Vec<String>,
    last_message_date: DateTime<Local>,
    last_message_preview: String,
    message_count: i64,
}

struct Message {
    message_id: i64,
    text: Option<String>,       // extracted from attributedBody
    sender: String,             // "Me" or handle.id
    is_from_me: bool,
    date: DateTime<Local>,
    date_read: Option<DateTime<Local>>,
    is_audio: bool,
    attachments: Vec<AttachmentInfo>,
    reply_to_guid: Option<String>,
    associated_emoji: Option<String>, // tapback/reaction
}

struct AttachmentInfo {
    attachment_id: i64,
    filename: Option<String>,
    mime_type: Option<String>,
    total_bytes: i64,
    transfer_name: Option<String>,
    is_sticker: bool,
}

struct SearchResult {
    message: Message,
    conversation: Conversation,
    context_before: Vec<Message>, // 2-3 messages before
    context_after: Vec<Message>,  // 2-3 messages after
}
```

### Database Schema (Apple's, read-only)

Key tables and relationships:
- `message` — ROWID, text, attributedBody, handle_id, is_from_me, date, cache_has_attachments, reply_to_guid, associated_message_emoji
- `handle` — ROWID, id (phone/email), service
- `chat` — ROWID, chat_identifier, display_name, style (43=group, 45=1-on-1)
- `attachment` — ROWID, filename, mime_type, total_bytes, transfer_name, is_sticker
- `chat_message_join` — chat_id, message_id, message_date
- `chat_handle_join` — chat_id, handle_id
- `message_attachment_join` — message_id, attachment_id

Current data volume: 237k messages, 2.4k chats, 2.8k handles, 32k attachments.

## Tauri Commands

| Command | Input | Output | Purpose |
|---------|-------|--------|---------|
| `check_db_access` | (none) | `DbStatus` | Verify chat.db is readable, helpful FDA error if not |
| `list_conversations` | offset, limit, sort_by | `Vec<Conversation>` | Paginated conversation list, sorted by recent |
| `get_messages` | chat_id, offset, limit | `Vec<Message>` | Paginated messages for a conversation |
| `search_messages` | query, chat_id (optional) | `Vec<SearchResult>` | Global or per-chat text search |
| `export_messages` | chat_id, start_id, end_id, include_attachments | `ExportResult` | Export range to JSON + attachment folder |
| `get_attachment` | attachment_id | file path | Serve attachment for inline preview |

## UI Layout

Two-panel layout:

**Left panel (280px fixed):**
- Search bar at top (global search)
- Scrollable conversation list: contact/group name, timestamp, last message preview
- Active conversation highlighted with accent border

**Right panel (flex):**
- Chat header: contact name, identifier, service type, "Search in chat" button, "Export Selection" button
- Message area: chat bubbles (sent right/blue, received left/gray), timestamps, date separators, sender names for group chats, attachment indicators
- Scroll-to-load-more for pagination (loads older messages upward)

**Export selection mode:**
- Activated by "Export Selection" button
- Click a message to set start point (amber highlight)
- Click another message to set end point (amber range)
- Bottom bar shows selection count with Cancel and "Export JSON" buttons
- "Export JSON" opens native file picker for destination folder

## Export Format

```
export_2026-03-17_+19145551234/
├── messages.json
└── attachments/
    ├── IMG_1234.heic
    └── voice_note.caf
```

`messages.json` contains:
```json
{
  "conversation": { "chat_identifier": "...", "display_name": "...", "participants": [...] },
  "export_range": { "start_date": "...", "end_date": "...", "message_count": 142 },
  "messages": [
    {
      "sender": "Me",
      "text": "Hey, want to grab lunch?",
      "date": "2025-01-15T10:30:22",
      "is_from_me": true,
      "attachments": [
        { "filename": "IMG_1234.heic", "mime_type": "image/heic", "total_bytes": 2048000 }
      ]
    }
  ]
}
```

## Build Stages

Each stage produces a working application.

### Stage 1: Foundation
- Tauri app scaffold within `src/msgs/`
- Rust SQLite layer — read-only connection to `chat.db`
- `attributedBody` parser — extract plain text from NSKeyedArchiver blobs
- Apple Core Data timestamp conversion
- `check_db_access` command — verify FDA, show helpful error if missing
- `list_conversations` command — paginated, sorted by most recent
- Frontend: conversation list panel (left side only)
- Type generation with `ts-rs` or `specta`

### Stage 2: Message View
- `get_messages` command — paginated messages for a chat
- Frontend: message view panel with chat bubbles
- Sent vs received styling, timestamps, date separators
- Scroll-to-load-more for older messages
- Group chat: sender name on each received bubble
- Attachment indicators (filename, type, size — no previews yet)

### Stage 3: Search
- `search_messages` command — LIKE-based search with optional chat_id filter
- Global search: results grouped by conversation with surrounding context
- In-chat search: filter within current conversation, highlight matches
- Click a search result to navigate to that message in context
- Debounced search input

### Stage 4: Export
- `export_messages` command — range-based export with attachment copying
- `get_attachment` command — serve attachment files for preview
- Export selection mode: click start message, click end message
- Amber highlight on selected range
- Native file picker for export destination
- JSON output with conversation metadata + messages + attachment files
- Progress indicator for large exports with many attachments

## Error Handling

- **No FDA**: `check_db_access` returns a clear error with instructions to grant Full Disk Access in System Settings
- **Missing chat.db**: Error with explanation (Messages may not be configured)
- **Corrupt attributedBody**: Fall back to empty text, log warning
- **Missing attachments**: Include metadata in JSON, skip file copy, note in export log
- **Large exports**: Async command with progress events pushed to frontend

## Testing Strategy

- **Rust unit tests**: attributedBody parser with various blob formats, date conversion, query building, export serialization
- **Rust integration tests**: Query against a test fixture copy of chat.db schema
- **UTF-8 safety**: Test attributedBody parser with multi-byte characters (emoji, Japanese, accented)
- **Frontend**: Minimal — UI rendering only, all logic is in Rust

## Security

- Read-only database access (SQLite `SQLITE_OPEN_READ_ONLY`)
- No network access needed
- Attachment file paths validated before copying (prevent path traversal)
- CSP configured to prevent loading external resources
- Tauri command allowlist restricted to the 6 defined commands
