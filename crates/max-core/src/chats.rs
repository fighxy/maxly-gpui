//! Pin, mute, and leaving a chat.
//!
//! Pin and mute live in the account `CONFIG` (`opcode` 22) as `ChatSettings` per chat, the
//! map `core.dll` stores: `favIndex` and `dontDisturbUntil`. A pin index of 0 is not pinned;
//! any larger value is, and a bigger one sorts first. `dontDisturbUntil` of -1 is muted with
//! no end, 0 is not muted, and a time still ahead is muted until then. Those values are what
//! the settings map holds; they are not yet checked against the live server.
//!
//! Leaving a group or a channel is `CHAT_LEAVE`. Deleting a dialog for ourselves is
//! `CHAT_DELETE` with `forAll` false.

use std::collections::BTreeMap;

use max_proto::Opcode;
use max_proto::schema::packets::chat::{chat_delete, chat_leave};
use max_proto::schema::packets::config;
use max_proto::schema::types::{ChatSettings, Configuration};
use max_transport::Transport;

use crate::Result;
use crate::messages::now_ms;

/// `favIndex` of a chat that is not pinned.
pub const UNPINNED: i64 = 0;
/// `dontDisturbUntil` of a chat muted with no end.
pub const MUTED_FOREVER: i64 = -1;
/// `dontDisturbUntil` of a chat whose notifications are on.
pub const NOT_MUTED: i64 = 0;

/// Pin order from account settings. `0` is not pinned.
pub fn pin_index(settings: &ChatSettings) -> i64 {
    settings.fav_index.unwrap_or(UNPINNED).max(UNPINNED)
}

/// Whether notifications for this chat are off right now.
pub fn is_muted(until: Option<i64>, now: i64) -> bool {
    match until {
        Some(t) if t < 0 => true,
        Some(t) if t > now => true,
        _ => false,
    }
}

/// Write one chat's settings. The answer may carry the stored row and a new config hash.
async fn write_settings(
    transport: &Transport,
    hash: &str,
    chat_id: i64,
    settings: ChatSettings,
) -> Result<config::Response> {
    let mut chats = BTreeMap::new();
    chats.insert(chat_id, settings);
    let params = config::Parameters {
        settings: Some(Configuration {
            hash: hash.to_string(),
            chats: Some(chats),
            ..Configuration::default()
        }),
        ..config::Parameters::default()
    };
    Ok(transport.call(Opcode::Config, &params).await?)
}

fn stored(response: &config::Response, chat_id: i64) -> Option<&ChatSettings> {
    response
        .chats
        .as_ref()
        .and_then(|chats| chats.get(&chat_id))
}

/// Pin or unpin `chat_id`. Returns the pin index now stored (0 when unpinned) and the config
/// hash from the answer, when the server sent one.
pub async fn set_pinned(
    transport: &Transport,
    hash: &str,
    chat_id: i64,
    pinned: bool,
) -> Result<(i64, Option<String>)> {
    let index = if pinned { now_ms() } else { UNPINNED };
    let response = write_settings(
        transport,
        hash,
        chat_id,
        ChatSettings {
            fav_index: Some(index),
            ..ChatSettings::default()
        },
    )
    .await?;
    let stored = stored(&response, chat_id).map(pin_index).unwrap_or(index);
    Ok((stored, response.hash))
}

/// Mute or unmute `chat_id`. Returns whether it is muted now, and a new config hash.
pub async fn set_muted(
    transport: &Transport,
    hash: &str,
    chat_id: i64,
    muted: bool,
) -> Result<(bool, Option<String>)> {
    let until = if muted { MUTED_FOREVER } else { NOT_MUTED };
    let response = write_settings(
        transport,
        hash,
        chat_id,
        ChatSettings {
            dont_disturb_until: Some(until),
            ..ChatSettings::default()
        },
    )
    .await?;
    let stored = stored(&response, chat_id)
        .map(|s| is_muted(s.dont_disturb_until, now_ms()))
        .unwrap_or(muted);
    Ok((stored, response.hash))
}

/// Leave a group or a channel (`CHAT_LEAVE`). The chat disappears from this account's list.
pub async fn leave(transport: &Transport, chat_id: i64) -> Result<()> {
    let params = chat_leave::Parameters { chat_id };
    let _: chat_leave::Response = transport.call(Opcode::ChatLeave, &params).await?;
    Ok(())
}

/// Delete a dialog for ourselves (`CHAT_DELETE`, `forAll` false).
pub async fn delete_for_me(transport: &Transport, chat_id: i64) -> Result<()> {
    let params = chat_delete::Parameters {
        chat_id,
        last_event_time: Some(now_ms()),
        for_all: Some(false),
    };
    let _: rmpv::Value = transport.call(Opcode::ChatDelete, &params).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_index_treats_missing_and_negative_as_unpinned() {
        assert_eq!(pin_index(&ChatSettings::default()), 0);
        assert_eq!(
            pin_index(&ChatSettings {
                fav_index: Some(0),
                ..ChatSettings::default()
            }),
            0
        );
        assert_eq!(
            pin_index(&ChatSettings {
                fav_index: Some(-3),
                ..ChatSettings::default()
            }),
            0
        );
        assert_eq!(
            pin_index(&ChatSettings {
                fav_index: Some(9),
                ..ChatSettings::default()
            }),
            9
        );
    }

    #[test]
    fn mute_is_forever_or_still_ahead() {
        assert!(!is_muted(None, 1_000));
        assert!(!is_muted(Some(0), 1_000));
        assert!(!is_muted(Some(500), 1_000));
        assert!(is_muted(Some(-1), 1_000));
        assert!(is_muted(Some(2_000), 1_000));
    }
}
