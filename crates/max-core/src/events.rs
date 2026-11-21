//! Server pushes the client acts on, decoded from their `core.dll` layouts.
//!
//! | opcode | event | layout |
//! |---|---|---|
//! | 128 | new or changed message | `NotificationMessageData::Payload`; `message.status` is `EDITED` or `REMOVED` for changes |
//! | 129 | someone is typing | `TypingData::Payload {chatId, userId, type}` |
//! | 130 | read mark moved | `NotificationMarkData::Response {chatId, userId, mark, unread, setAsUnread}` |
//! | 135 | chat changed or added | `NotificationChatData::Response {chat}` |
//! | 140 | messages deleted in a time range | `MessagesDeleteRangeData::Response {chat, startTime, endTime}` |
//! | 142 | messages deleted | `MessagesDeleteData::Response {chat, messageIds}` |
//! | 155 | reaction counters changed | `ReactionsChangedData::Response {chatId, messageId, totalCount, counters}` |
//! | 156 | our reaction changed | `YouReactedData::Response {chatId, messageId, reactionInfo}` |
//!
//! Typing is also sent from here (`MSG_TYPING` 65, `Typing::Send::Parameters {chatId, type}`);
//! the type names come from `Core::Typing::Interactor::doSend`: `TEXT` (the default), `AUDIO`,
//! `PHOTO`, `VIDEO`, `FILE`, `STICKER`, `VIDEO_MSG`, `LOCATION`.

use max_proto::Opcode;
use max_proto::schema::packets::events::{
    messages_delete_data, messages_delete_range_data, notification_chat_data,
    notification_mark_data, reactions_changed_data, typing_data, you_reacted_data,
};
use max_proto::schema::packets::typing;
use max_transport::{Response, Transport};

use crate::Result;
use crate::messages::{Message, NewMessage};

/// `Message.status` of an edited message.
pub const EDITED: &str = "EDITED";
/// `Message.status` of a deleted message.
pub const REMOVED: &str = "REMOVED";
/// Typing type for text.
pub const TYPING_TEXT: &str = "TEXT";

/// A chat as the server describes it.
pub type Chat = max_proto::schema::types::Chat;

/// What a push means for the client.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    /// A message arrived in a chat.
    Message(Box<NewMessage>),
    /// A message already shown was edited.
    Edited {
        /// Chat id.
        chat_id: i64,
        /// The message as it is now.
        message: Box<Message>,
    },
    /// Messages were deleted.
    Deleted {
        /// Chat id.
        chat_id: i64,
        /// Their ids.
        message_ids: Vec<i64>,
    },
    /// Messages in a time range were deleted (Unix ms, both ends included).
    DeletedRange {
        /// Chat id.
        chat_id: i64,
        /// First message time.
        from: i64,
        /// Last message time.
        to: i64,
    },
    /// Someone is typing.
    Typing {
        /// Chat id.
        chat_id: i64,
        /// Who.
        user_id: i64,
        /// `TEXT`, `AUDIO`, ...
        kind: Option<String>,
    },
    /// A read mark moved: ours (read on another device) or another participant's.
    Mark {
        /// Chat id.
        chat_id: i64,
        /// Whose mark.
        user_id: i64,
        /// Read up to this time, Unix ms.
        mark: i64,
        /// Unread count left, for our own mark.
        unread: Option<i32>,
    },
    /// A chat changed or was added.
    Chat(Box<Chat>),
    /// Reaction counters changed. `known_yours` is set by the "you reacted" push, which carries
    /// our emoji; the counters-only push does not.
    Reactions {
        /// Chat id.
        chat_id: i64,
        /// Message id.
        message_id: i64,
        /// Counters, and our emoji when `known_yours`.
        info: crate::messages::ReactionInfo,
        /// `info.your_reaction` is authoritative.
        known_yours: bool,
    },
}

/// Decode a push the client acts on; `None` for everything else.
pub fn decode(push: &Response) -> Option<ServerEvent> {
    Some(match push.header.opcode()? {
        Opcode::NotificationMessage => {
            let new: NewMessage = push.decode().ok()?;
            match new.message.status.as_deref() {
                Some(EDITED) => ServerEvent::Edited {
                    chat_id: new.chat_id,
                    message: Box::new(new.message),
                },
                Some(REMOVED) => ServerEvent::Deleted {
                    chat_id: new.chat_id,
                    message_ids: vec![new.message.id],
                },
                _ => ServerEvent::Message(Box::new(new)),
            }
        }
        Opcode::NotificationTyping => {
            let t: typing_data::Payload = push.decode().ok()?;
            ServerEvent::Typing {
                chat_id: t.chat_id,
                user_id: t.user_id,
                kind: t.type_,
            }
        }
        Opcode::NotificationMark => {
            let m: notification_mark_data::Response = push.decode().ok()?;
            ServerEvent::Mark {
                chat_id: m.chat_id,
                user_id: m.user_id,
                mark: m.mark,
                unread: m.unread,
            }
        }
        Opcode::NotificationChat => {
            let c: notification_chat_data::Response = push.decode().ok()?;
            ServerEvent::Chat(Box::new(c.chat))
        }
        Opcode::MessagesDelete => {
            let d: messages_delete_data::Response = push.decode().ok()?;
            ServerEvent::Deleted {
                chat_id: d.chat.id,
                message_ids: d.message_ids,
            }
        }
        Opcode::MessagesDeleteRange => {
            let d: messages_delete_range_data::Response = push.decode().ok()?;
            ServerEvent::DeletedRange {
                chat_id: d.chat.id,
                from: d.start_time,
                to: d.end_time,
            }
        }
        Opcode::ReactionsChanged => {
            let d: reactions_changed_data::Response = push.decode().ok()?;
            ServerEvent::Reactions {
                chat_id: d.chat_id,
                message_id: d.message_id,
                info: crate::messages::ReactionInfo {
                    counters: d.counters,
                    total_count: Some(d.total_count),
                    your_reaction: None,
                },
                known_yours: false,
            }
        }
        Opcode::YouReacted => {
            let d: you_reacted_data::Response = push.decode().ok()?;
            ServerEvent::Reactions {
                chat_id: d.chat_id,
                message_id: d.message_id,
                info: d.reaction_info.unwrap_or_default(),
                known_yours: true,
            }
        }
        _ => return None,
    })
}

/// Tell the chat we are typing (`MSG_TYPING`); `kind` is one of the type names above.
pub async fn send_typing(transport: &Transport, chat_id: i64, kind: &str) -> Result<()> {
    let params = typing::send::Parameters {
        chat_id,
        type_: kind.to_string(),
    };
    let _: rmpv::Value = transport.call(Opcode::TypingSend, &params).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use bytes::Bytes;
    use max_proto::payload::to_msgpack;
    use max_proto::{Cmd, Frame, PROTOCOL_VERSION};
    use rmpv::Value;

    use super::*;

    fn push(opcode: Opcode, body: &[(&str, Value)]) -> Response {
        let map = Value::Map(
            body.iter()
                .map(|(k, v)| (Value::from(*k), v.clone()))
                .collect(),
        );
        let payload = to_msgpack(&map).unwrap();
        let frame = Frame::new(
            PROTOCOL_VERSION,
            Cmd::Request,
            0,
            opcode.as_u16(),
            0,
            payload.clone(),
        )
        .unwrap();
        Response {
            header: frame.header,
            payload: Bytes::from(payload),
        }
    }

    fn message(id: i64, status: Option<&str>) -> Value {
        let mut m = vec![
            (Value::from("id"), Value::from(id)),
            (Value::from("time"), Value::from(5)),
            (Value::from("type"), Value::from("USER")),
            (Value::from("text"), Value::from("т")),
        ];
        if let Some(s) = status {
            m.push((Value::from("status"), Value::from(s)));
        }
        Value::Map(m)
    }

    fn chat(id: i64) -> Value {
        Value::Map(vec![(Value::from("id"), Value::from(id))])
    }

    #[test]
    fn message_status_tells_new_edited_and_removed_apart() {
        let new = push(
            Opcode::NotificationMessage,
            &[("chatId", 7.into()), ("message", message(1, None))],
        );
        assert!(matches!(decode(&new), Some(ServerEvent::Message(m)) if m.message.id == 1));
        let edited = push(
            Opcode::NotificationMessage,
            &[("chatId", 7.into()), ("message", message(1, Some(EDITED)))],
        );
        assert!(
            matches!(decode(&edited), Some(ServerEvent::Edited { chat_id: 7, message }) if message.id == 1)
        );
        let removed = push(
            Opcode::NotificationMessage,
            &[("chatId", 7.into()), ("message", message(1, Some(REMOVED)))],
        );
        assert!(matches!(
            decode(&removed),
            Some(ServerEvent::Deleted { chat_id: 7, message_ids }) if message_ids == [1]
        ));
    }

    #[test]
    fn typing_mark_and_deletes() {
        let t = push(
            Opcode::NotificationTyping,
            &[
                ("chatId", 7.into()),
                ("userId", 9.into()),
                ("type", "TEXT".into()),
            ],
        );
        assert!(matches!(
            decode(&t),
            Some(ServerEvent::Typing { chat_id: 7, user_id: 9, kind: Some(k) }) if k == "TEXT"
        ));
        let m = push(
            Opcode::NotificationMark,
            &[
                ("chatId", 7.into()),
                ("userId", 9.into()),
                ("mark", 100.into()),
                ("unread", 0.into()),
            ],
        );
        assert!(matches!(
            decode(&m),
            Some(ServerEvent::Mark {
                chat_id: 7,
                user_id: 9,
                mark: 100,
                unread: Some(0)
            })
        ));
        let d = push(
            Opcode::MessagesDelete,
            &[
                ("chat", chat(7)),
                ("messageIds", Value::Array(vec![1.into(), 2.into()])),
            ],
        );
        assert!(matches!(
            decode(&d),
            Some(ServerEvent::Deleted { chat_id: 7, message_ids }) if message_ids == [1, 2]
        ));
        let r = push(
            Opcode::MessagesDeleteRange,
            &[
                ("chat", chat(7)),
                ("startTime", 10.into()),
                ("endTime", 20.into()),
            ],
        );
        assert!(matches!(
            decode(&r),
            Some(ServerEvent::DeletedRange {
                chat_id: 7,
                from: 10,
                to: 20
            })
        ));
        let changed = push(
            Opcode::ReactionsChanged,
            &[
                ("chatId", 7.into()),
                ("messageId", 3.into()),
                ("totalCount", 2.into()),
                (
                    "counters",
                    Value::Array(vec![Value::Map(vec![
                        ("reaction".into(), "👍".into()),
                        ("count".into(), 2.into()),
                    ])]),
                ),
            ],
        );
        assert!(matches!(
            decode(&changed),
            Some(ServerEvent::Reactions {
                chat_id: 7,
                message_id: 3,
                known_yours: false,
                ref info,
            }) if info.total_count == Some(2)
                && info.counters.as_ref().is_some_and(|c| c.first().is_some_and(|c| c.reaction == "👍" && c.count == 2))
        ));
        let mine = push(
            Opcode::YouReacted,
            &[
                ("chatId", 7.into()),
                ("messageId", 3.into()),
                (
                    "reactionInfo",
                    Value::Map(vec![
                        ("yourReaction".into(), "👍".into()),
                        ("totalCount".into(), 1.into()),
                    ]),
                ),
            ],
        );
        assert!(matches!(
            decode(&mine),
            Some(ServerEvent::Reactions {
                known_yours: true,
                info,
                ..
            }) if info.your_reaction.as_deref() == Some("👍")
        ));
        let other = push(Opcode::NotificationPresence, &[("userId", 9.into())]);
        assert!(decode(&other).is_none());
    }
}
