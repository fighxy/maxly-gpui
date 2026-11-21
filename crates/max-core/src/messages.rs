//! Chat history, sending, editing and deleting text, replies, and new-message pushes.
//!
//! Layouts from `core.dll` ([`max_proto::schema`]). The history request values (`from` = now,
//! `backward` = 40, `forward` = 0, both window times 0) match PyMax and the shape of
//! `Core::ChatHistory::Interactor::doRequestMessages`; the exact values the official client
//! sends are still to be read from its log after a login (unverified).

use std::time::{SystemTime, UNIX_EPOCH};

use max_proto::Opcode;
use max_proto::schema::packets::chat::chat_history;
use max_proto::schema::packets::events::notification_message_data;
use max_proto::schema::packets::messaging::{delete, edit, send};
use max_proto::schema::types::OutgoingMessage;
use max_proto::schema::types::outgoing_message::OutgoingMessageLink;
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

/// `MessageLink` type of a reply (a `core.dll` string; PyMax sends it with `messageId` only).
pub const LINK_REPLY: &str = "REPLY";
/// `MessageLink` type of a forwarded message (a `core.dll` string).
pub const LINK_FORWARD: &str = "FORWARD";

/// Send a text message. `cid` is the client's id for the message, echoed back so the pending
/// copy can be matched; the current time in ms is a unique enough choice. `reply_to` is the id
/// of the message in the same chat this one answers (`link` of type `REPLY`).
pub async fn send_text(
    transport: &Transport,
    chat_id: i64,
    text: &str,
    cid: i64,
    reply_to: Option<i64>,
) -> Result<SendResponse> {
    send_composed(transport, chat_id, text, cid, reply_to, &[]).await
}

/// [`send_text`] with formatting elements (an animated emoji is an `ANIMOJI` element).
pub async fn send_composed(
    transport: &Transport,
    chat_id: i64,
    text: &str,
    cid: i64,
    reply_to: Option<i64>,
    elements: &[max_proto::schema::types::MessageElement],
) -> Result<SendResponse> {
    let params = send::Parameters {
        chat_id: Some(chat_id),
        notify: Some(true),
        message: OutgoingMessage {
            cid: Some(cid),
            text: Some(text.to_string()),
            link: reply_to.map(|message_id| OutgoingMessageLink {
                type_: LINK_REPLY.to_string(),
                message_id,
                ..OutgoingMessageLink::default()
            }),
            elements: (!elements.is_empty()).then(|| elements.to_vec()),
            ..OutgoingMessage::default()
        },
        ..send::Parameters::default()
    };
    Ok(transport.call(Opcode::MessagingSend, &params).await?)
}

/// Replace the text of our message (`MSG_EDIT`); the answer is the stored message. Only `text`
/// is sent: leaving out `elements` and `attachments` is meant to keep them as they are
/// (unverified on the real server).
pub async fn edit_text(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
    text: &str,
) -> Result<Message> {
    let params = edit::Parameters {
        chat_id,
        message_id,
        text: Some(text.to_string()),
        ..edit::Parameters::default()
    };
    let r: edit::Response = transport.call(Opcode::MessagingEdit, &params).await?;
    Ok(r.message)
}

/// Delete messages of a chat (`MSG_DELETE`): for everyone, or only for us with `for_me`.
/// Returns the ids the server deleted; the ones it lists as failed are left out.
pub async fn delete(
    transport: &Transport,
    chat_id: i64,
    message_ids: &[i64],
    for_me: bool,
) -> Result<Vec<i64>> {
    let params = delete::Parameters {
        chat_id,
        message_ids: message_ids.to_vec(),
        for_me: Some(for_me),
        ..delete::Parameters::default()
    };
    let r: delete::Response = transport.call(Opcode::MessagingDelete, &params).await?;
    let failed = r.failed_message_ids.unwrap_or_default();
    // An answer without ids is taken as "done as asked".
    let done = if r.message_ids.is_empty() {
        message_ids.to_vec()
    } else {
        r.message_ids
    };
    Ok(done.into_iter().filter(|id| !failed.contains(id)).collect())
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
/// `CHAT_MARK` type for "show this chat as unread". The other value beside [`READ_MESSAGE`]
/// in that converter (PyMax `ReadAction`). Unverified on the live server.
pub const SET_AS_UNREAD: &str = "SET_AS_UNREAD";

/// The server's answer to [`mark_read`]: the new read mark and unread count.
pub type MarkResponse = max_proto::schema::packets::chat::chat_mark::Response;

/// What a `max.ru` link points at (`LINK_INFO`): a chat, a person, a message, a sticker set or a call.
pub type LinkInfo = max_proto::schema::packets::link_info::Response;

/// Ask what `link` opens (`LINK_INFO`). A web page is not this: its preview arrives as a `SHARE`
/// attachment on the message.
pub async fn link_info(transport: &Transport, link: &str) -> Result<LinkInfo> {
    let params = max_proto::schema::packets::link_info::Parameters {
        link: link.to_string(),
    };
    Ok(transport.call(Opcode::LinkInfo, &params).await?)
}

/// `reactionType` of an emoji reaction (`MSG_REACTION`). The `id` is the emoji itself.
pub const REACTION_EMOJI: &str = "EMOJI";

/// Counters on a message, and which emoji is ours (`MessageReactionInfo`).
pub type ReactionInfo = max_proto::schema::types::MessageReactionInfo;

/// One person who reacted (`MessageReactionEntry`).
pub type ReactionEntry = max_proto::schema::types::MessageReactionEntry;

/// Put `emoji` on a message (`MSG_REACTION`). The answer is the message's counters afterwards.
pub async fn react(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
    emoji: &str,
) -> Result<ReactionInfo> {
    let params = max_proto::schema::packets::reactions::msg_react::Parameters {
        chat_id,
        post_id: None,
        message_id,
        reaction: max_proto::schema::types::MessageReaction {
            reaction_type: REACTION_EMOJI.to_string(),
            id: emoji.to_string(),
        },
    };
    let response: max_proto::schema::packets::reactions::msg_react::Response =
        transport.call(Opcode::MsgReact, &params).await?;
    Ok(response.reaction_info)
}

/// Take our reaction off a message (`MSG_CANCEL_REACTION`).
pub async fn cancel_reaction(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
) -> Result<ReactionInfo> {
    let params = max_proto::schema::packets::reactions::cancel_reaction::Parameters {
        chat_id,
        post_id: None,
        message_id,
    };
    let response: max_proto::schema::packets::reactions::cancel_reaction::Response =
        transport.call(Opcode::CancelReaction, &params).await?;
    Ok(response.reaction_info)
}

/// Counters for the given messages (`MSG_GET_REACTIONS`), keyed by message id.
pub async fn reactions(
    transport: &Transport,
    chat_id: i64,
    message_ids: &[i64],
) -> Result<std::collections::BTreeMap<i64, ReactionInfo>> {
    let params = max_proto::schema::packets::reactions::get_reactions::Parameters {
        chat_id,
        post_id: None,
        message_ids: message_ids.to_vec(),
    };
    let response: max_proto::schema::packets::reactions::get_reactions::Response =
        transport.call(Opcode::GetReactions, &params).await?;
    Ok(response.messages_reactions)
}

/// Who reacted (`MSG_GET_DETAILED_REACTIONS`): the counters and one entry per person.
pub async fn detailed_reactions(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
) -> Result<(ReactionInfo, Vec<ReactionEntry>)> {
    let params = max_proto::schema::packets::reactions::get_detailed_reactions::Parameters {
        chat_id,
        post_id: None,
        message_id,
        reaction: None,
        marker: None,
        count: Some(100),
    };
    let response: max_proto::schema::packets::reactions::get_detailed_reactions::Response =
        transport
            .call(Opcode::GetDetailedReactions, &params)
            .await?;
    Ok((
        response.reaction_info.unwrap_or_default(),
        response.reactions.unwrap_or_default(),
    ))
}

/// Emoji the chat allows (`REACTIONS_SETTINGS_GET_BY_CHAT_ID`). Empty when the server names none.
pub async fn chat_reaction_emoji(transport: &Transport, chat_id: i64) -> Result<Vec<String>> {
    let params = max_proto::schema::packets::reactions::get_chat_reactions::Parameters {
        chat_ids: vec![chat_id],
    };
    let response: max_proto::schema::packets::reactions::get_chat_reactions::Response =
        transport.call(Opcode::GetChatReactions, &params).await?;
    let ids = response
        .chat_reactions_settings
        .into_iter()
        .flatten()
        .find(|s| s.is_active && s.chat_id.is_none_or(|id| id == chat_id))
        .and_then(|s| s.reaction_ids)
        .unwrap_or_default();
    Ok(ids.into_iter().filter(|s| !s.is_empty()).collect())
}

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

/// Mark `chat_id` unread (`CHAT_MARK`, `type` [`SET_AS_UNREAD`]). `message_id` is the newest
/// message we know, when the history is loaded.
pub async fn mark_unread(
    transport: &Transport,
    chat_id: i64,
    message_id: Option<i64>,
) -> Result<MarkResponse> {
    let params = max_proto::schema::packets::chat::chat_mark::Parameters {
        chat_id,
        mark: Some(now_ms()),
        message_id,
        type_: Some(SET_AS_UNREAD.to_string()),
    };
    Ok(transport.call(Opcode::ChatMark, &params).await?)
}
