//! Chat history, sending text, and new-message pushes.
//!
//! Layouts from `core.dll` ([`max_proto::schema`]). The history request values (`from` = now,
//! `backward` = 40, `forward` = 0, both window times 0) match PyMax and the shape of
//! `Core::ChatHistory::Interactor::doRequestMessages`; the exact values the official client
//! sends are still to be read from its log after a login (unverified).

use std::time::{SystemTime, UNIX_EPOCH};

use max_proto::Opcode;
use max_proto::schema::packets::chat::chat_history;
use max_proto::schema::packets::events::notification_message_data;
use max_proto::schema::packets::messaging::send;
use max_proto::schema::types::OutgoingMessage;
use max_transport::{Response, Transport};

use crate::Result;

/// A message (`Api::OneMe::Types::Message`).
pub type Message = max_proto::schema::types::Message;
/// The answer to [`send_text`] (`Messaging::Send::Response`): the stored message and chat.
pub type SendResponse = send::Response;
/// A `NOTIF_MESSAGE` push (`Events::NotificationMessageData::Payload`).
pub type NewMessage = notification_message_data::Payload;

/// How many messages one history page asks for.
pub const HISTORY_PAGE: i32 = 40;

/// Unix time in ms.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Up to `backward` messages of `chat_id` before `from` (Unix ms), oldest first.
pub async fn history(
    transport: &Transport,
    chat_id: i64,
    from: i64,
    backward: i32,
) -> Result<Vec<Message>> {
    let params = chat_history::Parameters {
        chat_id,
        post_id: None,
        from,
        backward,
        backward_time: 0,
        forward: 0,
        forward_time: 0,
        item_type: None,
    };
    let r: chat_history::Response = transport.call(Opcode::ChatHistory, &params).await?;
    let mut messages = r.messages;
    messages.sort_by_key(|m| (m.time, m.id));
    Ok(messages)
}

/// Send a text message. `cid` is the client's id for the message, echoed back so the pending
/// copy can be matched; the current time in ms is a unique enough choice.
pub async fn send_text(
    transport: &Transport,
    chat_id: i64,
    text: &str,
    cid: i64,
) -> Result<SendResponse> {
    let params = send::Parameters {
        chat_id: Some(chat_id),
        notify: Some(true),
        message: OutgoingMessage {
            cid: Some(cid),
            text: Some(text.to_string()),
            ..OutgoingMessage::default()
        },
        ..send::Parameters::default()
    };
    Ok(transport.call(Opcode::MessagingSend, &params).await?)
}

/// The new message in a push, if it is a `NOTIF_MESSAGE`.
pub fn new_message(push: &Response) -> Option<NewMessage> {
    if push.header.opcode() == Some(Opcode::NotificationMessage) {
        push.decode().ok()
    } else {
        None
    }
}

/// `CHAT_MARK` type for "read up to this message" (`core.dll` converter `FUN_18122eb60`; the
/// value set matches PyMax `ReadAction.READ_MESSAGE`).
pub const READ_MESSAGE: &str = "READ_MESSAGE";

/// The server's answer to [`mark_read`]: the new read mark and unread count.
pub type MarkResponse = max_proto::schema::packets::chat::chat_mark::Response;

/// Mark `chat_id` as read up to `message_id` (`CHAT_MARK`, `type` `READ_MESSAGE`, `mark` = now).
pub async fn mark_read(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
) -> Result<MarkResponse> {
    let params = max_proto::schema::packets::chat::chat_mark::Parameters {
        chat_id,
        mark: Some(now_ms()),
        message_id: Some(message_id),
        type_: Some(READ_MESSAGE.to_string()),
    };
    Ok(transport.call(Opcode::ChatMark, &params).await?)
}
