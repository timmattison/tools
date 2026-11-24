# msgs — iMessage Browser & Exporter

## Overview

A Tauri v2 desktop application for browsing, searching, and exporting iMessage conversations. Reads directly from macOS's `~/Library/Messages/chat.db` in read-only mode. Built as part of the Buffalo Tools monorepo.

## Requirements

- **macOS 13 (Ventura) or later** — minimum for Tauri v2 and the `chat.db` schema observed
- **Tauri v2** — uses capabilities/permissions model (not v1 allowlist)
- **Full Disk Access** — must be granted to the app in System Settings > Privacy & Security
- **Apple Silicon and Intel** supported

## Problem

Messages.app has poor text selection and no export capability. The iMessage SQLite database contains all the data but requires manual SQL queries to access. This app provides a GUI for browsing, searching, and exporting message ranges with attachments.

## Architecture

**Approach: Direct SQLite, read-only.** No shadow database or import step. The Rust backend opens `chat.db` read-only and queries Apple's schema directly.

### Directory Layout

The Tauri app requires a non-standard layout compared to other `src/*` members. Structure:

```
src/msgs/
├── Cargo.toml          # Workspace member, Tauri app crate
├── tauri.conf.json     # Tauri configuration
├── capabilities/       # Tauri v2 permissions
├── icons/
├── build.rs            # Tauri build script
├── src/
│   ├── main.rs         # Tauri app entry point
│   ├── commands.rs     # Tauri command handlers
│   ├── db.rs           # SQLite query layer
│   ├── parser.rs       # attributedBody parser
│   ├── export.rs       # Export engine
│   ├── error.rs        # Error types
│   └── types.rs        # Data model structs
└── frontend/           # Vanilla TS + Vite
    ├── package.json
    ├── index.html
    ├── src/
    │   ├── main.ts
    │   ├── conversations.ts
    │   ├── messages.ts
    │   ├── search.ts
    │   └── export.ts
    └── src/styles/
        └── main.css    # Plain CSS with custom properties
```

The workspace `Cargo.toml` uses `members = ["src/*"]` which will pick up `src/msgs/Cargo.toml` directly. The `Cargo.toml` at `src/msgs/` is the Tauri app crate (it contains the `tauri` dependency and build script). The frontend lives in `src/msgs/frontend/` and is referenced by `tauri.conf.json`'s `frontendDist` / `devUrl` settings.

### Backend (Rust)

All business logic lives in Rust:

- **DB Layer**: Read-only SQLite connection to `~/Library/Messages/chat.db` via `rusqlite` with `SQLITE_OPEN_READ_ONLY`. Requires Full Disk Access granted to the app.
- **attributedBody Parser**: Extracts plain text from NSKeyedArchiver/streamtyped blobs. 99.9% of messages (235k of 237k) store text in `attributedBody`, not the `text` column. See [attributedBody Parsing](#attributedbody-parsing) section for details. The parser is a distinct module with fallback handling for unrecognized blob formats.
- **Date Conversion**: Apple Core Data timestamps are nanoseconds since 2001-01-01. Convert via `date / 1_000_000_000 + 978_307_200` to Unix epoch. Serialized as ISO 8601 with timezone offset (e.g., `2025-01-15T10:30:22-05:00`).
- **Text Cache**: On app launch, the backend extracts text from all `attributedBody` blobs and caches in a local SQLite database (`~/.local/share/msgs/text-cache.db`) with an FTS5 virtual table. This enables fast full-text search without modifying `chat.db`. The cache is rebuilt when the source fingerprint of `chat.db` changes (the count of joined message rows, the highest message ROWID, and the latest edit date) or on manual refresh.
- **Query Builder**: Constructs SQL against Apple's schema (`message`, `chat`, `handle`, `attachment`, and their join tables).
- **Export Engine**: Serializes message ranges to JSON and copies attachment files to an export folder.
- **Search Engine**: FTS5 queries against the text cache, optionally filtered to a specific chat_id. Falls back to in-memory search if cache is stale.

### Frontend (Vanilla TypeScript + Vite)

Pure view layer — no application state beyond UI concerns (scroll position, selection mode, form inputs). All data fetched via Tauri commands. Styled with plain CSS using CSS custom properties for theming (dark theme).

### Type Safety

Rust structs generate TypeScript types via `specta`. No manual type synchronization.

### IPC

All frontend-to-backend communication via typed Tauri commands. Every command returns `Result<T, MsgsError>`. Backend pushes updates via Tauri events where needed (e.g., text cache build progress, export progress).

### Version Information

Uses `buildinfo::version_string!()` macro like all other tools in the repo. Version (including git hash and dirty status) is exposed via the native About window and a `get_version` Tauri command.

## attributedBody Parsing

The `attributedBody` column contains serialized `NSAttributedString` data in Apple's typedstream/NSKeyedArchiver format. The parser extracts the plain text portion.

**Format:**
1. Find the `NSString` marker in the blob
2. Skip 4 header bytes (`\x01\x94\x84\x01`)
3. Read the type byte. It must be `0x2b` (`+`).
4. Read the length as a typedstream integer: a first byte below `0x80` is the length, `0x81` is followed by an i16 little-endian, and `0x82` is followed by an i32 little-endian
5. Read that many bytes as UTF-8 text

**Failure behavior:**
- If the blob has no `NSString` marker, a different header, a different type byte, or a length that is not valid, the parser returns `None` and logs a warning with the offset of the problem
- The parser module is isolated so versioned strategies can be added if Apple changes the format in future macOS releases

**Testing:** Unit tests include blobs with ASCII, multi-byte UTF-8 (emoji, Japanese, accented characters), empty strings, texts of 127, 128, 300, and 70,000 bytes, and synthetic malformed blobs.

## Data Model

### Error Types

`MsgsError` implements `Serialize` by hand as its message (`serializer.serialize_str(&self.to_string())`). Tauri sends that string as the rejection value of the invoke, and the frontend shows it.

```rust
#[derive(Debug, thiserror::Error)]
enum MsgsError {
    #[error("Full Disk Access not granted. Open System Settings > Privacy & Security > Full Disk Access and add this app.")]
    NoFullDiskAccess,
    #[error("chat.db not found at ~/Library/Messages/chat.db. Is Messages configured?")]
    DatabaseNotFound,
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Failed to parse message body: {0}")]
    ParseError(String),
    #[error("Export failed: {0}")]
    ExportError(String),
    #[error("Attachment not found: {0}")]
    AttachmentNotFound(String),
    #[error("Text cache error: {0}")]
    CacheError(String),
}
```

### Core Structs

```rust
struct Conversation {
    chat_id: i64,
    chat_identifier: String,    // phone number, email, or URN
    display_name: Option<String>, // group chat name, or None for 1-on-1
    is_group: bool,             // style 43 = group, 45 = 1-on-1
    participants: Vec<String>,  // handle.id values (phone/email)
    last_message_date: Option<DateTime<FixedOffset>>,
    last_message_preview: String, // max 100 chars, truncated via chars() per UTF-8 safety rules
    message_count: i64,
}

struct Message {
    message_id: i64,
    text: Option<String>,       // extracted from attributedBody
    sender: String,             // "Me" or handle.id (phone/email)
    is_from_me: bool,
    date: Option<DateTime<FixedOffset>>,
    date_read: Option<DateTime<FixedOffset>>,
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

struct ExportResult {
    export_path: String,
    message_count: i64,
    attachment_count: i64,
}

struct DbStatus {
    accessible: bool,
    message_count: Option<i64>,
    error: Option<String>,
}
```

**Contact name resolution:** Handle IDs are phone numbers or email addresses. Resolving these to contact names requires macOS Contacts framework access (a separate permission). This is explicitly out of scope for the initial build. Group chat `display_name` from the `chat` table is used where available. Contact resolution may be added as a future enhancement.

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

All commands return `Result<T, MsgsError>`.

| Command | Input | Output | Purpose |
|---------|-------|--------|---------|
| `check_db_access` | (none) | `DbStatus` | Verify chat.db is readable, helpful FDA error if not |
| `list_conversations` | offset, limit, sort_by | `Vec<Conversation>` | Paginated conversation list, sorted by recent |
| `get_messages` | chat_id, offset, limit | `Vec<Message>` | Paginated messages for a conversation |
| `search_messages` | query, chat_id (optional), offset, limit | `Vec<SearchResult>` | FTS5 search, global or per-chat |
| `export_messages` | chat_id, start_message_id, end_message_id, include_attachments, export_path | `ExportResult` | Export the messages between two messages to JSON + attachment folder |
| `get_version` | (none) | `String` | Version string with git hash and dirty status |
| `rebuild_text_cache` | (none) | (none) | Force rebuild of the FTS5 text cache |

### Attachments

The message view shows each attachment as one line with its name and size. The webview does not load attachment files, thus the app needs no asset protocol and no file-system plugin. The export copies the attachment files (see the export section). Inline display of images is on the wishlist (`WISHLIST.md`).

### Export Range

The `export_messages` command takes the ids of the first and the last selected message, in either order, and never a date. A date from the frontend has only whole seconds, so a `BETWEEN` on dates drops the end message and each later message of the same second, and a date in nanoseconds exceeds the exact integer range of a JavaScript number. The backend reads the position of each id in the chat (`message_date`, then `ROWID`) and selects with the row-value range `(message_date, ROWID) BETWEEN (start date, start id) AND (end date, end id)` through `chat_message_join` filtered by `chat_id`. Both ends are included. An id that is not in the chat is an export error.

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

**Styling:** Plain CSS with CSS custom properties for theming. Dark theme only (matches typical developer preference and Messages.app dark mode). No CSS framework.

### Keyboard Shortcuts

- `Cmd+F` — focus global search bar
- `Cmd+Shift+F` — focus in-chat search (when a conversation is open)
- `Up/Down` — navigate conversation list
- `Escape` — cancel export selection mode, clear search
- `Enter` — open selected conversation

## Export Format

```
export_2026-03-17_+19145551234/
├── messages.json
└── attachments/
    ├── 101-IMG_1234.heic
    └── 102-voice_note.caf
```

Each copy is named `<attachment_id>-<file name>`. Messages keeps each attachment in its own GUID directory, thus the same file name occurs many times, and the id keeps the copies apart. The export copies each attachment id once.

`messages.json` contains:
```json
{
  "conversation": { "chat_identifier": "...", "display_name": "...", "participants": [...] },
  "export_range": { "start_date": "2025-01-15T10:30:22-05:00", "end_date": "2025-03-17T14:22:00-05:00", "message_count": 142 },
  "messages": [
    {
      "sender": "Me",
      "text": "Hey, want to grab lunch?",
      "date": "2025-01-15T10:30:22-05:00",
      "is_from_me": true,
      "attachments": [
        { "attachment_id": 101, "filename": "IMG_1234.heic", "mime_type": "image/heic", "total_bytes": 2048000, "exported_file": "attachments/101-IMG_1234.heic" }
      ]
    }
  ]
}
```

`filename` keeps the original path. `exported_file` is the relative path of the copy, or `null` when the export made no copy (attachments off, a path outside the root, or a missing file). `attachment_count` counts the files copied.

All timestamps are ISO 8601 with timezone offset.

## Build Stages

Each stage produces a working application.

### Stage 1: Foundation
- Tauri v2 app scaffold within `src/msgs/` with the directory layout described above
- `buildinfo` integration for version string
- Rust SQLite layer — read-only connection to `chat.db`
- `attributedBody` parser module — extract plain text from NSKeyedArchiver blobs
- Apple Core Data timestamp conversion
- `MsgsError` enum with all variants
- `check_db_access` command — verify FDA, show helpful error if missing
- `list_conversations` command — paginated, sorted by most recent
- Frontend: conversation list panel (left side only)
- Type generation with `specta`
- Tauri v2 capabilities configuration

### Stage 2: Message View
- `get_messages` command — paginated messages for a chat
- Frontend: message view panel with chat bubbles
- Sent vs received styling, timestamps, date separators
- Scroll-to-load-more for older messages
- Group chat: sender name on each received bubble
- Attachment indicators (filename, type, size — no previews yet)
- Keyboard navigation (Up/Down for conversations, Enter to open)

### Stage 3: Search
- Text cache builder — extract all `attributedBody` text into local FTS5 database on launch
- `search_messages` command — FTS5 search with optional chat_id filter
- `rebuild_text_cache` command — force cache rebuild
- Global search: results grouped by conversation with surrounding context
- In-chat search: filter within current conversation, highlight matches
- Click a search result to navigate to that message in context
- Debounced search input
- `Cmd+F` / `Cmd+Shift+F` keyboard shortcuts

### Stage 4: Export
- `export_messages` command — date-range-based export with attachment copying
- Export selection mode: click start message, click end message
- Amber highlight on selected range
- Native file picker for export destination
- JSON output with conversation metadata + messages + attachment files
- Progress events pushed to frontend for large exports
- Escape to cancel selection mode

## Error Handling

All Tauri commands return `Result<T, MsgsError>`. The frontend maps error variants to user-facing messages:

- **NoFullDiskAccess**: Shown on app launch with step-by-step instructions to enable FDA
- **DatabaseNotFound**: Explanation that Messages may not be configured on this machine
- **DatabaseError**: Generic database access failures with technical detail
- **ParseError**: Logged as warning, message shown with "[unable to read message]" placeholder
- **ExportError**: Shown inline in the export bar with option to retry
- **AttachmentNotFound**: Metadata included in export JSON, file copy skipped, noted in export summary
- **CacheError**: Search degrades gracefully, prompts user to rebuild cache

## Testing Strategy

- **Rust unit tests**: attributedBody parser with various blob formats (ASCII, emoji, Japanese, accented, empty, malformed), date conversion, export serialization, error enum coverage
- **Rust integration tests**: Query against a test fixture copy of chat.db schema with synthetic data
- **UTF-8 safety**: Parser and preview truncation tested with multi-byte characters per repo guidelines
- **Frontend**: Minimal — UI rendering only, all logic is in Rust

## Security

- Read-only database access (SQLite `SQLITE_OPEN_READ_ONLY`)
- No network access needed
- Attachment file paths validated to be within `~/Library/Messages/Attachments/` after `..` and symlinks resolve (both the path and the root are canonicalized before the comparison) and before the export copies them (path traversal prevention)
- CSP configured to prevent loading external resources
- Tauri v2 capabilities restricted to `core:default` and `dialog:default` (the save dialog of the export). The app has no file-system plugin and no asset protocol
- Text cache database stored in `~/.local/share/msgs/` with no sensitive data beyond message text
