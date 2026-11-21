//! Emoji assets: the animated set the official client draws with ThorVG.
//!
//! Komet loads it with `ASSETS_UPDATE` `{type: ANIMOJI_SET, sync: 0}` and then
//! `ASSETS_GET_BY_IDS`. This server answers that update with `{sync: 0, sections: []}`. The
//! sticker update with `sync` omitted returns the account snapshot (`NEW_STICKER_SETS`,
//! `RECENT`, including `emojiList`), so the same omission is used for the other asset types.
//! Named sections are then read with the desktop `ASSETS_GET` (`count` as a signed int). Set
//! ids still come from `animojiSetIds`, or from `animojis` / `animojiIds` on a set.

use std::collections::BTreeMap;

use max_proto::Opcode;
use max_proto::payload::to_msgpack;
use max_proto::schema::packets::assets::get_asset::common::Parameters as GetAsset;
use max_proto::schema::packets::assets::get_by_ids::{animoji, common};
use max_proto::schema::types::message_element::AttributesMap;
use max_proto::schema::types::{Animoji, MessageElement};
use max_transport::Transport;
use rmpv::Value;

use crate::Result;

/// One animated emoji the picker can insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimojiFace {
    /// Server id (`Animoji.id`), sent as the element's `entityId`.
    pub id: i64,
    /// The set it belongs to (`animojiSetId`), when the server named one.
    pub set_id: Option<i64>,
    /// The characters written into the message.
    pub emoji: String,
    /// Lottie file (`lottieUrl`, or `lottiePlayUrl` when that is the only address).
    pub lottie_url: String,
}

/// Asset type of a set of animated emoji (`core.dll`, beside `STICKER_SET`).
const ANIMOJI_SET: &str = "ANIMOJI_SET";
/// Asset type of one animated emoji.
const ANIMOJI: &str = "ANIMOJI";
/// How many sets the picker asks for.
const MAX_SETS: usize = 12;
/// How many animated emoji the picker shows. Komet loads them in batches of 100.
const MAX_FACES: usize = 100;

/// Ids and ready faces gathered from one assets response.
#[derive(Debug, Default)]
struct Found {
    set_ids: Vec<i64>,
    /// Animoji id and the set it was listed under.
    face_ids: Vec<(i64, Option<i64>)>,
    ready: Vec<AnimojiFace>,
    /// `Section.id` values, used to page the set when the update only names the sections.
    section_ids: Vec<String>,
    /// Sticker-set ids from a sticker snapshot. Loaded afterwards in case a set lists animoji.
    pack_ids: Vec<i64>,
    /// Short description of the first sections, kept for the empty-catalog message.
    sketch: Vec<String>,
}

impl Found {
    fn any(&self) -> bool {
        !self.set_ids.is_empty() || !self.face_ids.is_empty() || !self.ready.is_empty()
    }
}

/// The animated emoji of the account.
pub async fn animoji_faces(transport: &Transport) -> Result<Vec<AnimojiFace>> {
    let mut found = Found::default();
    let mut last_err = None;
    let mut saw_ok = false;
    // `ANIMOJI_SET` with `sync` 0 is a 17-byte `{sync: 0, sections: []}`. The sticker snapshot
    // with `sync` omitted is the reply that actually carries sections (`NEW_STICKER_SETS`,
    // `RECENT` with `emojiList`). The same omission is asked for the other asset types, then
    // each named section is read with the desktop `ASSETS_GET` (`count` as a signed int).
    let probes = [
        Ask::Update {
            type_: "STICKER",
            sync: false,
        },
        Ask::Update {
            type_: "EMOJI",
            sync: false,
        },
        Ask::Update {
            type_: ANIMOJI,
            sync: false,
        },
        Ask::Update {
            type_: "REACTION",
            sync: false,
        },
        Ask::Update {
            type_: "ANIMOJI_SETS",
            sync: false,
        },
    ];
    for probe in probes {
        absorb(transport, &probe, &mut found, &mut saw_ok, &mut last_err).await;
        if found.any() {
            break;
        }
    }
    if !found.any() {
        let pages: Vec<String> = found.section_ids.iter().take(4).cloned().collect();
        for section in pages {
            absorb(
                transport,
                &Ask::Page {
                    section: section.clone(),
                    from: 0,
                },
                &mut found,
                &mut saw_ok,
                &mut last_err,
            )
            .await;
            if found.any() {
                break;
            }
        }
    }
    if !found.any() {
        for (type_, section) in [
            (ANIMOJI_SET, "SYSTEM"),
            (ANIMOJI, "SYSTEM"),
            ("ANIMOJI_SETS", "SYSTEM"),
        ] {
            absorb(
                transport,
                &Ask::Asset {
                    asset_type: type_,
                    section: section.to_string(),
                },
                &mut found,
                &mut saw_ok,
                &mut last_err,
            )
            .await;
            if found.any() {
                break;
            }
        }
    }
    if !found.any() && !found.pack_ids.is_empty() {
        let ids: Vec<i64> = found.pack_ids.iter().copied().take(MAX_SETS).collect();
        match by_ids(transport, "STICKER_SET", &ids).await {
            Ok(value) => inspect_sticker_sets(&value, &mut found),
            Err(e) => note_err(&mut found, "STICKER_SET", &e),
        }
    }
    if !found.ready.is_empty() {
        found.ready.truncate(MAX_FACES);
        return Ok(found.ready);
    }
    let mut wanted = std::mem::take(&mut found.face_ids);
    if wanted.is_empty() && !found.set_ids.is_empty() {
        let set_ids: Vec<i64> = found.set_ids.iter().copied().take(MAX_SETS).collect();
        match by_ids(transport, ANIMOJI_SET, &set_ids).await {
            Ok(value) => {
                let mut loaded = Found::default();
                harvest(&value, &mut loaded, true);
                if !loaded.ready.is_empty() {
                    loaded.ready.truncate(MAX_FACES);
                    return Ok(loaded.ready);
                }
                wanted.extend(loaded.face_ids);
            }
            Err(e) => last_err = Some(e),
        }
        // Komet treats `animojiUpdates` keys as face ids when a set lists none.
        if wanted.is_empty() {
            wanted.extend(set_ids.into_iter().map(|id| (id, None)));
        }
    }
    wanted.truncate(MAX_FACES);
    if wanted.is_empty() {
        if let Some(e) = last_err.filter(|_| !saw_ok) {
            return Err(e);
        }
        return Err(crate::CoreError::Assets(format!(
            "в ответе нет набора ({})",
            empty_note(&found)
        )));
    }
    let ids: Vec<i64> = wanted.iter().map(|(id, _)| *id).collect();
    let faces = animojis(transport, &ids).await?;
    let set_of: BTreeMap<i64, Option<i64>> = wanted.into_iter().collect();
    let drawn: Vec<AnimojiFace> = faces
        .into_iter()
        .filter_map(|face| {
            let from_set = set_of.get(&face.id).copied().flatten();
            face_of(face, from_set)
        })
        .collect();
    if drawn.is_empty() {
        return Err(crate::CoreError::Assets(format!(
            "в ответе нет лиц ({})",
            empty_note(&found)
        )));
    }
    Ok(drawn)
}

/// `ANIMOJI` elements for the picks whose emoji still occurs in `text`, in order.
/// Offsets are UTF-16 code units, as the server stores `from` and `length`.
pub fn animoji_elements(text: &str, picks: &[AnimojiFace]) -> Vec<MessageElement> {
    let mut left: Vec<&AnimojiFace> = picks.iter().collect();
    let mut out = Vec::new();
    let mut byte = 0;
    while byte < text.len() {
        let Some(at) = left
            .iter()
            .position(|face| text[byte..].starts_with(&face.emoji))
        else {
            byte += text[byte..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let face = left.remove(at);
        let from = utf16_len(&text[..byte]);
        let length = utf16_len(&face.emoji);
        byte += face.emoji.len();
        let (Ok(from), Ok(length)) = (i16::try_from(from), i16::try_from(length)) else {
            continue;
        };
        out.push(MessageElement {
            entity_id: Some(face.id),
            type_: ANIMOJI.to_string(),
            from: Some(from),
            length: Some(length),
            attributes: Some(AttributesMap {
                animoji_set_id: face.set_id,
                animoji_lottie_url: Some(face.lottie_url.clone()),
                ..AttributesMap::default()
            }),
            ..MessageElement::default()
        });
    }
    out
}

/// Picks that still occur in `text`, in the order those characters appear.
pub fn claim_animoji(text: &str, picks: &[AnimojiFace]) -> Vec<AnimojiFace> {
    let mut left = picks.to_vec();
    let mut kept = Vec::new();
    let mut byte = 0;
    while byte < text.len() {
        let Some(at) = left
            .iter()
            .position(|face| text[byte..].starts_with(&face.emoji))
        else {
            byte += text[byte..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let face = left.remove(at);
        byte += face.emoji.len();
        kept.push(face);
    }
    kept
}

fn face_of(face: Animoji, from_set: Option<i64>) -> Option<AnimojiFace> {
    let emoji = face.emoji;
    if emoji.is_empty() {
        return None;
    }
    let lottie_url = face
        .lottie_play_url
        .filter(|url| !url.is_empty())
        .or(face.lottie_url.filter(|url| !url.is_empty()))?;
    Some(AnimojiFace {
        id: face.id,
        set_id: face.set_id.or(from_set),
        emoji,
        lottie_url,
    })
}

enum Ask {
    /// Opcode 27. `sync` is left out when `sync` is false.
    Update { type_: &'static str, sync: bool },
    /// Opcode 26 with the desktop parameter names. `count` is a signed int.
    Asset {
        asset_type: &'static str,
        section: String,
    },
    /// Opcode 26 with only the section, the cursor and the page size.
    Page { section: String, from: i64 },
}

impl Ask {
    fn label(&self) -> String {
        match self {
            Ask::Update { type_, sync } => {
                if *sync {
                    (*type_).to_string()
                } else {
                    format!("{type_} nosync")
                }
            }
            Ask::Asset { asset_type, section } => format!("asset {asset_type}/{section}"),
            Ask::Page { section, from } => format!("page {section}@{from}"),
        }
    }

    /// Sticker-set ids are animoji sets only on an animoji-typed request. A sticker snapshot
    /// keeps them when its own section type contains `ANIMOJI`.
    fn keeps_sets(&self) -> bool {
        match self {
            Ask::Update { type_, .. } => type_.contains("ANIMOJI"),
            Ask::Asset { asset_type, .. } => asset_type.contains("ANIMOJI"),
            Ask::Page { section, .. } => section.contains("ANIMOJI"),
        }
    }

    fn opcode(&self) -> Opcode {
        match self {
            Ask::Asset { .. } | Ask::Page { .. } => Opcode::AssetsGet,
            Ask::Update { .. } => Opcode::AssetsUpdate,
        }
    }
}

async fn absorb(
    transport: &Transport,
    probe: &Ask,
    found: &mut Found,
    saw_ok: &mut bool,
    last_err: &mut Option<crate::CoreError>,
) {
    let before = found.set_ids.len() + found.face_ids.len() + found.ready.len();
    match ask(transport, probe).await {
        Ok((value, bytes)) => {
            *saw_ok = true;
            harvest(&value, found, probe.keeps_sets());
            if found.set_ids.len() + found.face_ids.len() + found.ready.len() == before {
                note_reply(found, &probe.label(), &value, bytes);
            }
        }
        Err(e) => {
            note_err(found, &probe.label(), &e);
            *last_err = Some(e);
        }
    }
}

fn note_err(found: &mut Found, label: &str, err: &crate::CoreError) {
    if found.sketch.len() >= 12 {
        return;
    }
    let text: String = err.to_string().chars().take(80).collect();
    found.sketch.push(format!("{label} err {text}"));
}

async fn ask(transport: &Transport, query: &Ask) -> Result<(Value, usize)> {
    let payload = match query {
        Ask::Update { type_, sync } => to_msgpack(&update_value(type_, *sync))?,
        Ask::Asset { asset_type, section } => asset_payload(Some(asset_type), section, 0)?,
        Ask::Page { section, from } => asset_payload(None, section, *from)?,
    };
    let response = transport.request(query.opcode().as_u16(), payload).await?;
    let bytes = response.payload.len();
    Ok((response.value()?, bytes))
}

fn update_value(type_: &str, sync: bool) -> Value {
    let mut entries = vec![(Value::from("type"), Value::from(type_))];
    if sync {
        entries.push((Value::from("sync"), Value::from(0_i64)));
    }
    Value::Map(entries)
}

/// Desktop `ASSETS_GET`: `count` is a signed int, `from` is an int64. A positive number stored
/// in an `rmpv::Value` is written as an unsigned int, and opcode 26 rejects that.
fn asset_payload(asset_type: Option<&str>, section: &str, from: i64) -> Result<Vec<u8>> {
    Ok(to_msgpack(&GetAsset {
        asset_type: asset_type.map(str::to_string),
        section_id: Some(section.to_string()),
        from: Some(from),
        count: Some(100),
        ..GetAsset::default()
    })?)
}

async fn by_ids(transport: &Transport, type_: &str, ids: &[i64]) -> Result<Value> {
    let params = common::Parameters {
        type_: type_.to_string(),
        ids: ids.to_vec(),
    };
    let response = transport
        .request(Opcode::AssetsGetByIds.as_u16(), to_msgpack(&params)?)
        .await?;
    Ok(response.value()?)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nil",
        Value::Array(_) => "array",
        Value::Map(_) => "map",
        Value::String(_) => "str",
        Value::Integer(_) => "int",
        _ => "other",
    }
}

fn note_reply(found: &mut Found, label: &str, value: &Value, bytes: usize) {
    if found.sketch.len() >= 12 {
        return;
    }
    let Value::Map(entries) = value else {
        found
            .sketch
            .push(format!("{label} {bytes}b {}", value_kind(value)));
        return;
    };
    let sync = field_i64(entries, "sync")
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".to_string());
    let sections = match field(entries, "sections") {
        Some(Value::Array(items)) => items.len().to_string(),
        Some(_) => "other".to_string(),
        None => "-".to_string(),
    };
    let mut extra = Vec::new();
    for (key, child) in entries {
        let Some(name) = key.as_str() else {
            continue;
        };
        if name == "sync" || name == "sections" {
            continue;
        }
        extra.push(match child {
            Value::Array(items) => array_preview(name, items),
            Value::Map(items) => format!("{name}#{}", items.len()),
            _ => name.to_string(),
        });
        if extra.len() == 6 {
            break;
        }
    }
    let tail = if extra.is_empty() {
        String::new()
    } else {
        format!(" {}", extra.join("+"))
    };
    found
        .sketch
        .push(format!("{label} {bytes}b sync={sync} n={sections}{tail}"));
}

fn empty_note(found: &Found) -> String {
    if found.sketch.is_empty() && found.section_ids.is_empty() {
        return "пустой ответ".to_string();
    }
    let mut parts = Vec::new();
    if !found.sketch.is_empty() {
        parts.push(found.sketch.join(" | "));
    }
    if !found.section_ids.is_empty() {
        let shown: Vec<&str> = found.section_ids.iter().take(12).map(String::as_str).collect();
        parts.push(format!("секции {}", shown.join(", ")));
    }
    parts.join("; ")
}

fn harvest(value: &Value, found: &mut Found, sets: bool) {
    let Value::Map(entries) = value else {
        return;
    };
    for (key, child) in entries {
        match key.as_str() {
            Some("animojiSets" | "animojiSetIds") => take_sets(child, found),
            Some("animojis" | "animojiIds" | "animoji") => take_faces(child, None, found),
            Some("stickerSets") if sets => take_sets(child, found),
            Some("stickers") if !sets => take_faces(child, None, found),
            Some("animojiSetUpdates") => take_map_keys(child, &mut found.set_ids),
            Some("animojiUpdates") => {
                let mut ids = Vec::new();
                take_map_keys(child, &mut ids);
                for id in ids {
                    push_face(&mut found.face_ids, id, None);
                }
            }
            Some("sections") => {
                let Value::Array(sections) = child else {
                    continue;
                };
                for section in sections {
                    take_section(section, found, sets);
                }
            }
            _ => {}
        }
    }
}

/// One section of an assets update. Set ids may sit in `animojiSetIds` or, on an animoji
/// update, in `stickerSets` — that is the key the sticker snapshot uses for its own sets.
fn take_section(section: &Value, found: &mut Found, sets: bool) {
    let Value::Map(entries) = section else {
        return;
    };
    let id = field_str(entries, "id").unwrap_or_default();
    let type_ = field_str(entries, "type").unwrap_or_default();
    if !id.is_empty() && found.section_ids.len() < 24 && !found.section_ids.iter().any(|have| have == &id) {
        found.section_ids.push(id.clone());
    }
    let animoji = type_.contains("ANIMOJI") || type_.is_empty();
    let mut names = Vec::new();
    for (key, child) in entries {
        let Some(name) = key.as_str() else {
            continue;
        };
        names.push(match child {
            Value::Array(items) => format!("{name}#{}", items.len()),
            Value::Map(entries) => format!("{name}#{}", entries.len()),
            _ => name.to_string(),
        });
        match name {
            "animojiSets" | "animojiSetIds" => take_sets(child, found),
            "animojis" | "animojiIds" | "animoji" => take_faces(child, None, found),
            "stickerSets" if type_.contains("ANIMOJI") || (sets && type_.is_empty()) => {
                take_sets(child, found);
            }
            "stickerSets" => take_pack_ids(child, found),
            "emojiList" => take_emoji_list(child, found),
            "recentsList" => take_recent_assets(child, found),
            "stickers" if type_.contains("ANIMOJI") || (!sets && type_.is_empty()) => {
                take_faces(child, None, found);
            }
            "ids" if animoji => {
                if sets {
                    take_sets(child, found);
                } else {
                    take_faces(child, None, found);
                }
            }
            "recentEmojiList" => take_recent_emoji(child, found),
            _ => {}
        }
    }
    if found.sketch.len() < 8 {
        let label = if type_.is_empty() {
            id
        } else {
            format!("{id}/{type_}")
        };
        found.sketch.push(format!("{label}:{}", names.join("+")));
    }
}

fn array_preview(name: &str, items: &[Value]) -> String {
    let texts: Vec<&str> = items.iter().filter_map(Value::as_str).take(4).collect();
    if texts.is_empty() {
        format!("{name}#{}", items.len())
    } else {
        format!("{name}[{}]", texts.join(","))
    }
}

fn brief(entries: &[(Value, Value)]) -> String {
    let mut parts = Vec::new();
    for (key, child) in entries.iter().take(8) {
        let Some(name) = key.as_str() else {
            continue;
        };
        let shown = match child.as_i64() {
            Some(n) => format!("{name}={n}"),
            None => match child.as_str() {
                Some(text) => {
                    let text: String = text.chars().take(20).collect();
                    format!("{name}={text}")
                }
                None => match child {
                    Value::Array(items) => format!("{name}#{}", items.len()),
                    Value::Map(items) => format!("{name}#{}", items.len()),
                    _ => name.to_string(),
                },
            },
        };
        parts.push(shown);
    }
    parts.join("+")
}

fn take_pack_ids(value: &Value, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        let id = item.as_i64().or_else(|| {
            let Value::Map(entries) = item else {
                return None;
            };
            field_i64(entries, "id")
        });
        if let Some(id) = id {
            push_id(&mut found.pack_ids, id);
        }
        if found.pack_ids.len() >= MAX_SETS {
            break;
        }
    }
}

/// `emojiList` is not on the desktop `Section` struct. The live sticker snapshot carries one
/// element there, so its keys are kept and any animoji fields are harvested.
fn take_emoji_list(value: &Value, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items.iter().take(3) {
        let Value::Map(entries) = item else {
            if found.sketch.len() < 12 {
                found
                    .sketch
                    .push(format!("emojiList {}", value_kind(item)));
            }
            continue;
        };
        if found.sketch.len() < 12 {
            found.sketch.push(format!("emojiList {}", brief(entries)));
        }
        if let Some(ids) = field(entries, "animojiSetIds").or_else(|| field(entries, "animojiSets"))
        {
            take_sets(ids, found);
        }
        let set_id = field_i64(entries, "setId");
        if let Some(ids) = field(entries, "animojiIds").or_else(|| field(entries, "animojis")) {
            take_faces(ids, set_id, found);
        }
        let emoji = field_str(entries, "emoji").filter(|text| !text.is_empty());
        let lottie_url = field_str(entries, "lottiePlayUrl")
            .filter(|url| !url.is_empty())
            .or_else(|| field_str(entries, "lottieUrl").filter(|url| !url.is_empty()));
        if let (Some(id), Some(emoji), Some(lottie_url)) =
            (field_i64(entries, "id"), emoji, lottie_url)
        {
            if !found.ready.iter().any(|face| face.id == id) {
                found.ready.push(AnimojiFace {
                    id,
                    set_id,
                    emoji,
                    lottie_url,
                });
            }
        } else if field_str(entries, "type")
            .is_some_and(|type_| type_.contains("ANIMOJI"))
        {
            if let Some(id) = field_i64(entries, "id") {
                push_face(&mut found.face_ids, id, set_id);
            }
        }
    }
}

fn take_recent_assets(value: &Value, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        let Value::Map(entries) = item else {
            continue;
        };
        if !field_str(entries, "type").is_some_and(|type_| type_.contains("ANIMOJI")) {
            continue;
        }
        if let Some(id) = field_i64(entries, "id").or_else(|| field_i64(entries, "animojiId")) {
            push_face(&mut found.face_ids, id, None);
        }
    }
}

/// Sticker sets from the snapshot. Animoji ids are kept; the set name and keys are kept when
/// the set lists none, so the next empty-catalog message names the packs.
fn inspect_sticker_sets(value: &Value, found: &mut Found) {
    let Value::Map(entries) = value else {
        if found.sketch.len() < 12 {
            found
                .sketch
                .push(format!("STICKER_SET {}", value_kind(value)));
        }
        return;
    };
    let Some(Value::Array(sets)) = field(entries, "stickerSets") else {
        if found.sketch.len() < 12 {
            found
                .sketch
                .push(format!("STICKER_SET {}", brief(entries)));
        }
        return;
    };
    for set in sets.iter().take(MAX_SETS) {
        let Value::Map(fields) = set else {
            continue;
        };
        let name = field_str(fields, "name").unwrap_or_default();
        if found.sketch.len() < 12 {
            found
                .sketch
                .push(format!("набор {name}:{}", brief(fields)));
        }
        let set_id = field_i64(fields, "id");
        if let Some(ids) = field(fields, "animojiIds").or_else(|| field(fields, "animojis")) {
            take_faces(ids, set_id, found);
        }
        if let Some(ids) = field(fields, "animojiSetIds") {
            take_sets(ids, found);
        }
    }
}

fn take_recent_emoji(value: &Value, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        let Value::Map(entries) = item else {
            continue;
        };
        let type_ = field_str(entries, "type").unwrap_or_default();
        if !type_.is_empty() && type_ != ANIMOJI {
            continue;
        }
        if let Some(id) = field_i64(entries, "id") {
            push_face(&mut found.face_ids, id, None);
        }
    }
}

fn take_sets(value: &Value, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        if let Some(id) = item.as_i64() {
            push_id(&mut found.set_ids, id);
            continue;
        }
        let Value::Map(entries) = item else {
            continue;
        };
        let Some(id) = field_i64(entries, "id") else {
            continue;
        };
        push_id(&mut found.set_ids, id);
        if let Some(ids) = field(entries, "animojiIds") {
            take_faces(ids, Some(id), found);
        }
        // Komet reads this key as well: a set may list face ids in `animojis`.
        if let Some(ids) = field(entries, "animojis") {
            take_faces(ids, Some(id), found);
        }
    }
}

fn take_faces(value: &Value, set_id: Option<i64>, found: &mut Found) {
    let Value::Array(items) = value else {
        return;
    };
    for item in items {
        if let Some(id) = item.as_i64() {
            push_face(&mut found.face_ids, id, set_id);
            continue;
        }
        let Value::Map(entries) = item else {
            continue;
        };
        let Some(id) = field_i64(entries, "id") else {
            continue;
        };
        let set_id = field_i64(entries, "setId").or(set_id);
        let emoji = field_str(entries, "emoji").unwrap_or_default();
        let lottie_url = field_str(entries, "lottiePlayUrl")
            .filter(|url| !url.is_empty())
            .or_else(|| field_str(entries, "lottieUrl").filter(|url| !url.is_empty()));
        if let Some(lottie_url) = lottie_url.filter(|_| !emoji.is_empty()) {
            if !found.ready.iter().any(|face| face.id == id) {
                found.ready.push(AnimojiFace {
                    id,
                    set_id,
                    emoji,
                    lottie_url,
                });
            }
        } else {
            push_face(&mut found.face_ids, id, set_id);
        }
    }
}

fn take_map_keys(value: &Value, ids: &mut Vec<i64>) {
    let Value::Map(entries) = value else {
        return;
    };
    for (key, _) in entries {
        if let Some(id) = key.as_i64() {
            push_id(ids, id);
        }
    }
}

fn field<'a>(entries: &'a [(Value, Value)], name: &str) -> Option<&'a Value> {
    entries
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

fn field_i64(entries: &[(Value, Value)], name: &str) -> Option<i64> {
    field(entries, name).and_then(Value::as_i64)
}

fn field_str(entries: &[(Value, Value)], name: &str) -> Option<String> {
    field(entries, name).and_then(Value::as_str).map(str::to_string)
}

fn push_id(ids: &mut Vec<i64>, id: i64) {
    if !ids.contains(&id) {
        ids.push(id);
    }
}

fn push_face(ids: &mut Vec<(i64, Option<i64>)>, id: i64, set_id: Option<i64>) {
    if !ids.iter().any(|(have, _)| *have == id) {
        ids.push((id, set_id));
    }
}

async fn animojis(transport: &Transport, ids: &[i64]) -> Result<Vec<Animoji>> {
    let params = common::Parameters {
        type_: ANIMOJI.to_string(),
        ids: ids.to_vec(),
    };
    let response: animoji::Response = transport.call(Opcode::AssetsGetByIds, &params).await?;
    Ok(response.animojis)
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(emoji: &str) -> AnimojiFace {
        AnimojiFace {
            id: 7,
            set_id: Some(3),
            emoji: emoji.to_string(),
            lottie_url: "https://e/a.json".into(),
        }
    }

    #[test]
    fn an_animoji_covers_its_emoji_in_utf16() {
        let text = "Привет 👋";
        let elements = animoji_elements(text, &[face("👋")]);
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].type_, "ANIMOJI");
        assert_eq!(elements[0].entity_id, Some(7));
        assert_eq!(elements[0].from, Some(7));
        assert_eq!(elements[0].length, Some(2));
        let url = elements[0]
            .attributes
            .as_ref()
            .and_then(|a| a.animoji_lottie_url.clone());
        assert_eq!(url.as_deref(), Some("https://e/a.json"));
    }

    #[test]
    fn one_pick_marks_the_first_copy_only() {
        let elements = animoji_elements("👋👋", &[face("👋")]);
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].from, Some(0));
        assert!(animoji_elements("нет", &[face("👋")]).is_empty());
    }

    #[test]
    fn ids_come_from_a_set_list_and_a_section() {
        let value = Value::Map(vec![
            (
                Value::from("animojiSets"),
                Value::Array(vec![Value::from(1_i64), Value::from(2_i64)]),
            ),
            (
                Value::from("sections"),
                Value::Array(vec![Value::Map(vec![(
                    Value::from("animojiSetIds"),
                    Value::Array(vec![Value::from(2_i64), Value::from(4_i64)]),
                )])]),
            ),
        ]);
        let mut found = Found::default();
        harvest(&value, &mut found, true);
        assert_eq!(found.set_ids, vec![1, 2, 4]);
    }

    #[test]
    fn a_set_snapshot_keeps_ids_stored_as_sticker_sets() {
        let value = Value::Map(vec![(
            Value::from("sections"),
            Value::Array(vec![Value::Map(vec![
                (Value::from("id"), Value::from("POPULAR")),
                (Value::from("type"), Value::from("ANIMOJI_SET")),
                (
                    Value::from("stickerSets"),
                    Value::Array(vec![Value::from(5_i64), Value::from(6_i64)]),
                ),
            ])]),
        )]);
        let mut found = Found::default();
        harvest(&value, &mut found, true);
        assert_eq!(found.set_ids, vec![5, 6]);
        assert_eq!(found.section_ids, vec!["POPULAR".to_string()]);
    }

    #[test]
    fn a_set_lists_its_faces_under_animojis() {
        let value = Value::Map(vec![(
            Value::from("animojiSets"),
            Value::Array(vec![Value::Map(vec![
                (Value::from("id"), Value::from(3_i64)),
                (
                    Value::from("animojis"),
                    Value::Array(vec![Value::from(9_i64), Value::from(10_i64)]),
                ),
            ])]),
        )]);
        let mut found = Found::default();
        harvest(&value, &mut found, true);
        assert_eq!(found.face_ids, vec![(9, Some(3)), (10, Some(3))]);
    }

    #[test]
    fn a_face_object_is_ready_to_draw() {
        let value = Value::Map(vec![(
            Value::from("animojis"),
            Value::Array(vec![Value::Map(vec![
                (Value::from("id"), Value::from(9_i64)),
                (Value::from("emoji"), Value::from("👋")),
                (Value::from("lottieUrl"), Value::from("https://e/a.json")),
            ])]),
        )]);
        let mut found = Found::default();
        harvest(&value, &mut found, false);
        assert_eq!(found.ready.len(), 1);
        assert_eq!(found.ready[0].emoji, "👋");
        assert_eq!(found.ready[0].lottie_url, "https://e/a.json");
    }

    #[test]
    fn a_section_list_survives_the_value_round_trip() {
        let payload = Value::Map(vec![
            (Value::from("sync"), Value::from(5_i64)),
            (
                Value::from("sections"),
                Value::Array(vec![Value::Map(vec![
                    (Value::from("id"), Value::from("SYSTEM")),
                    (Value::from("type"), Value::from("ANIMOJI_SETS")),
                    (
                        Value::from("animojiSetIds"),
                        Value::Array(vec![Value::from(3_i64), Value::from(4_i64)]),
                    ),
                ])]),
            ),
        ]);
        let bytes = max_proto::payload::to_msgpack(&payload).unwrap_or_default();
        let back: Value = max_proto::payload::from_msgpack(&bytes).unwrap_or(Value::Nil);
        let mut found = Found::default();
        harvest(&back, &mut found, false);
        assert_eq!(found.set_ids, vec![3, 4]);
    }

}
