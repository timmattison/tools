export interface Message {
  message_id: number;
  text: string | null;
  sender: string;
  is_from_me: boolean;
  date: string;
  date_read: string | null;
  is_audio: boolean;
  attachments: AttachmentInfo[];
  reply_to_guid: string | null;
  associated_emoji: string | null;
}

export interface AttachmentInfo {
  attachment_id: number;
  filename: string | null;
  mime_type: string | null;
  total_bytes: number;
  transfer_name: string | null;
  is_sticker: boolean;
}

export interface Conversation {
  chat_id: number;
  chat_identifier: string;
  display_name: string | null;
  is_group: boolean;
  participants: string[];
  last_message_date: string;
  last_message_preview: string;
  message_count: number;
}

export interface SearchResult {
  message: Message;
  conversation: Conversation;
  context_before: Message[];
  context_after: Message[];
}

export interface DbStatus {
  accessible: boolean;
  message_count: number | null;
  error: string | null;
}

export interface ExportResult {
  export_path: string;
  message_count: number;
  attachment_count: number;
}
