//! Attachments: what a message carries, downloading photos and files, uploading and sending them.
//!
//! Layouts from `core.dll` ([`max_proto::schema`]): `PHOTO_UPLOAD {count}` -> `{url}`,
//! `FILE_UPLOAD {count}` -> `{info: [{fileId, url, token}]}`, `FILE_DOWNLOAD {fileId, chatId,
//! messageId}` -> `{url}`, `NOTIF_ATTACH {fileId, error}`. The desktop builds photo uploads as
//! `multipart/form-data` (`form-data; name="%1"; filename="%2"`, `network.dll`) and reads a JSON
//! answer with `photos` (`core.dll` `FUN_18151d2a0`). The form field name `file`, the raw-body
//! file upload with `Content-Disposition` / `Content-Range`, and waiting for `NOTIF_ATTACH`
//! follow PyMax and are unverified until a logged-in test.

use bytes::Bytes;
use max_proto::Opcode;
use max_proto::schema::Any;
use max_proto::schema::packets::attachments::{file_download, file_upload, photo_upload};
use max_proto::schema::packets::events::notification_attach_data;
use max_proto::schema::packets::messaging::send;
use max_proto::schema::types::OutgoingMessage;
use max_transport::{Response, Transport, http_tls_config};
use rmpv::Value;

use crate::messages::{Message, SendResponse};
use crate::{CoreError, Result};

/// Largest download the client accepts (photos, files).
pub const MAX_DOWNLOAD: usize = 200 * 1024 * 1024;

/// What a message attachment is, as far as the client shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum Attachment {
    /// A photo at `url` (`baseUrl`).
    Photo {
        /// Photo id, if given.
        photo_id: Option<i64>,
        /// Image address.
        url: String,
        /// Pixel width (0 if unknown).
        width: i32,
        /// Pixel height (0 if unknown).
        height: i32,
    },
    /// A file, downloaded through [`file_download_url`].
    File {
        /// File id.
        file_id: i64,
        /// File name.
        name: String,
        /// Size in bytes.
        size: i64,
    },
    /// A video, shown by its preview image (`VIDEO` attach: `videoId`, `thumbnail`, `width`,
    /// `height`, `duration` in ms; names in `core.dll`, the unit from PyMax).
    Video {
        /// Video id, for `VIDEO_PLAY`.
        video_id: i64,
        /// Preview image address, if given.
        thumbnail: Option<String>,
        /// Pixel width (0 if unknown).
        width: i32,
        /// Pixel height (0 if unknown).
        height: i32,
        /// Length in ms (0 if unknown).
        duration: i64,
    },
    /// A sticker image.
    Sticker {
        /// Image address.
        url: String,
    },
    /// A link preview (`SHARE`: `url`, `title`, `description`, `host`, `image`, `isSensitive`).
    /// The server attaches it to a message; the image stays hidden while `sensitive`.
    Share {
        /// Where a click goes.
        url: String,
        /// Page or chat title. Empty when the server sent none.
        title: String,
        /// Short text under the title.
        description: String,
        /// Site shown under the text. Empty when the server sent none.
        host: String,
        /// Preview image, when there is one.
        image: Option<String>,
        /// The image is not shown until the user asks.
        sensitive: bool,
    },
    /// Anything else, by its `_type`.
    Other {
        /// The `_type` value.
        kind: String,
    },
}

fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_map()?
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn text(v: &Value, key: &str) -> Option<String> {
    field(v, key).and_then(Value::as_str).map(str::to_string)
}

fn int(v: &Value, key: &str) -> Option<i64> {
    field(v, key).and_then(Value::as_i64)
}

fn flag(v: &Value, key: &str) -> bool {
    match field(v, key) {
        Some(Value::Boolean(on)) => *on,
        Some(v) => v.as_i64() == Some(1),
        None => false,
    }
}

/// A `SHARE` attachment: the link card the server stored with the message.
fn share(v: &Value) -> Attachment {
    let url = text(v, "url").unwrap_or_default();
    let image = field(v, "image").and_then(|image| {
        text(image, "baseUrl")
            .or_else(|| text(image, "url"))
            .or_else(|| text(image, "previewUrl"))
            .or_else(|| image.as_str().map(str::to_string))
    });
    if url.is_empty() && image.is_none() && text(v, "title").is_none() {
        return Attachment::Other {
            kind: "SHARE".into(),
        };
    }
    Attachment::Share {
        url,
        title: text(v, "title").unwrap_or_default(),
        description: text(v, "description").unwrap_or_default(),
        host: text(v, "host").unwrap_or_default(),
        image: image.filter(|u| !u.is_empty()),
        sensitive: flag(v, "isSensitive"),
    }
}

/// The attachments of a message.
pub fn attachments(m: &Message) -> Vec<Attachment> {
    m.attaches
        .iter()
        .flatten()
        .map(|Any(v)| {
            let kind = text(v, "_type").unwrap_or_default();
            match kind.as_str() {
                "PHOTO" => match text(v, "baseUrl").or_else(|| text(v, "url")) {
                    Some(url) => Attachment::Photo {
                        photo_id: int(v, "photoId"),
                        url,
                        width: int(v, "width")
                            .and_then(|w| i32::try_from(w).ok())
                            .unwrap_or(0),
                        height: int(v, "height")
                            .and_then(|h| i32::try_from(h).ok())
                            .unwrap_or(0),
                    },
                    None => Attachment::Other { kind },
                },
                "FILE" => Attachment::File {
                    file_id: int(v, "fileId").unwrap_or(0),
                    name: text(v, "name").unwrap_or_else(|| "файл".into()),
                    size: int(v, "size").unwrap_or(0),
                },
                "VIDEO" => Attachment::Video {
                    video_id: int(v, "videoId").unwrap_or(0),
                    thumbnail: text(v, "thumbnail").filter(|u| !u.is_empty()),
                    width: int(v, "width")
                        .and_then(|w| i32::try_from(w).ok())
                        .unwrap_or(0),
                    height: int(v, "height")
                        .and_then(|h| i32::try_from(h).ok())
                        .unwrap_or(0),
                    duration: int(v, "duration").unwrap_or(0),
                },
                "STICKER" => match text(v, "url") {
                    Some(url) => Attachment::Sticker { url },
                    None => Attachment::Other { kind },
                },
                "SHARE" => share(v),
                _ => Attachment::Other { kind },
            }
        })
        .collect()
}

/// An HTTP failure with its whole cause chain (reqwest's own text hides TLS and connect causes).
fn http_error(e: reqwest::Error) -> CoreError {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    CoreError::Http(text)
}

/// HTTPS for media and uploads, with the certificate rules of ADR-011 and the desktop user agent.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    /// A client that sends `user_agent` (`ClientVersion::http_user_agent`).
    pub fn new(user_agent: &str) -> Result<Http> {
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(http_tls_config()?)
            .user_agent(user_agent)
            .build()
            .map_err(http_error)?;
        Ok(Http { client })
    }

    /// GET `url`, at most `max` bytes.
    pub async fn download(&self, url: &str, max: usize) -> Result<Bytes> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(http_error)?;
        if resp.content_length().is_some_and(|n| n > max as u64) {
            return Err(CoreError::Http(format!("{url}: larger than {max} bytes")));
        }
        let body = resp.bytes().await.map_err(http_error)?;
        if body.len() > max {
            return Err(CoreError::Http(format!("{url}: larger than {max} bytes")));
        }
        Ok(body)
    }
}

/// The address to download a file attachment from (`FILE_DOWNLOAD`).
pub async fn file_download_url(
    transport: &Transport,
    chat_id: i64,
    message_id: i64,
    file_id: i64,
) -> Result<String> {
    let params = file_download::Parameters {
        file_id,
        chat_id,
        post_id: None,
        message_id,
    };
    let r: file_download::Response = transport.call(Opcode::FileDownload, &params).await?;
    Ok(r.url)
}

/// Upload an image and return its `photoToken` for [`OutgoingAttachment::Photo`].
pub async fn upload_photo(
    transport: &Transport,
    http: &Http,
    data: Bytes,
    file_name: &str,
    mime: &str,
) -> Result<String> {
    let r: photo_upload::Response = transport
        .call(
            Opcode::PhotoUpload,
            &photo_upload::Parameters {
                count: 1,
                profile: None,
            },
        )
        .await?;
    let part = reqwest::multipart::Part::stream(data)
        .file_name(file_name.to_string())
        .mime_str(mime)
        .map_err(http_error)?;
    let form = reqwest::multipart::Form::new().part("file", part);
    let body: serde_json::Value = http
        .client
        .post(&r.url)
        .multipart(form)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(http_error)?
        .json()
        .await
        .map_err(http_error)?;
    photo_token(&body)
}

/// The token in a photo upload answer: `{"photos": {"<id>": {"token": "..."}}}`.
pub fn photo_token(body: &serde_json::Value) -> Result<String> {
    body.get("photos")
        .and_then(serde_json::Value::as_object)
        .and_then(|photos| photos.values().next())
        .and_then(|p| p.get("token"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| CoreError::Http(format!("photo upload answer without a token: {body}")))
}

/// Upload a file and return its `fileId`. The server confirms processing with a
/// `NOTIF_ATTACH` push ([`attach_ready`]); send the message after that.
pub async fn upload_file(
    transport: &Transport,
    http: &Http,
    data: Bytes,
    file_name: &str,
) -> Result<i64> {
    let r: file_upload::Response = transport
        .call(Opcode::FileUpload, &file_upload::Parameters { count: 1 })
        .await?;
    let info = r
        .info
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Http("FILE_UPLOAD answer without upload info".into()))?;
    let size = data.len();
    let last = size.saturating_sub(1);
    http.client
        .post(&info.url)
        .header(
            "Content-Disposition",
            format!("attachment; filename={}", urlencoding(file_name)),
        )
        .header("Content-Range", format!("0-{last}/{size}"))
        .header("Content-Type", "application/octet-stream")
        .body(data)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(http_error)?;
    Ok(info.file_id)
}

/// Percent-encode a file name for a header (RFC 3986 unreserved characters stay).
fn urlencoding(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The file id a `NOTIF_ATTACH` push confirms, with the server's error if processing failed.
pub fn attach_ready(push: &Response) -> Option<(i64, Option<String>)> {
    if push.header.opcode() != Some(Opcode::NotificationAttach) {
        return None;
    }
    let r: notification_attach_data::Response = push.decode().ok()?;
    r.file_id.map(|id| (id, r.error))
}

/// An attachment to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutgoingAttachment {
    /// A photo uploaded with [`upload_photo`].
    Photo {
        /// `photoToken`.
        token: String,
    },
    /// A file uploaded with [`upload_file`].
    File {
        /// `fileId`.
        file_id: i64,
    },
}

impl OutgoingAttachment {
    fn to_value(&self) -> Value {
        match self {
            OutgoingAttachment::Photo { token } => Value::Map(vec![
                ("_type".into(), "PHOTO".into()),
                ("photoToken".into(), token.as_str().into()),
            ]),
            OutgoingAttachment::File { file_id } => Value::Map(vec![
                ("_type".into(), "FILE".into()),
                ("fileId".into(), Value::from(*file_id)),
            ]),
        }
    }
}

/// Send a message with one attachment and optional text.
pub async fn send_attachment(
    transport: &Transport,
    chat_id: i64,
    attachment: &OutgoingAttachment,
    caption: Option<&str>,
    cid: i64,
) -> Result<SendResponse> {
    let params = send::Parameters {
        chat_id: Some(chat_id),
        notify: Some(true),
        message: OutgoingMessage {
            cid: Some(cid),
            text: caption.map(str::to_string),
            attaches: Some(vec![Any(attachment.to_value())]),
            ..OutgoingMessage::default()
        },
        ..send::Parameters::default()
    };
    Ok(transport.call(Opcode::MessagingSend, &params).await?)
}

/// Whether a file name looks like an image the server takes as a photo.
pub fn is_photo_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".heic"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// MIME type for a photo file name.
pub fn photo_mime(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    match lower.rsplit('.').next() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("heic") => "image/heic",
        _ => "image/jpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(k, v)| (Value::from(*k), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn reads_photo_file_sticker_and_other() {
        let m = Message {
            attaches: Some(vec![
                Any(map(&[
                    ("_type", "PHOTO".into()),
                    ("baseUrl", "https://i.example/p".into()),
                    ("width", 800.into()),
                    ("height", 600.into()),
                    ("photoId", 5.into()),
                ])),
                Any(map(&[
                    ("_type", "FILE".into()),
                    ("fileId", 9.into()),
                    ("name", "a.pdf".into()),
                    ("size", 1234.into()),
                ])),
                Any(map(&[
                    ("_type", "STICKER".into()),
                    ("url", "https://i.example/s".into()),
                ])),
                Any(map(&[("_type", "POLL".into())])),
            ]),
            ..Message::default()
        };
        assert_eq!(
            attachments(&m),
            [
                Attachment::Photo {
                    photo_id: Some(5),
                    url: "https://i.example/p".into(),
                    width: 800,
                    height: 600
                },
                Attachment::File {
                    file_id: 9,
                    name: "a.pdf".into(),
                    size: 1234
                },
                Attachment::Sticker {
                    url: "https://i.example/s".into()
                },
                Attachment::Other {
                    kind: "POLL".into()
                },
            ]
        );
    }

    #[test]
    fn reads_a_link_preview() {
        let image = map(&[("baseUrl", "https://i.example/p".into())]);
        let m = Message {
            attaches: Some(vec![
                Any(map(&[
                    ("_type", "SHARE".into()),
                    ("url", "https://example.com/a".into()),
                    ("title", "Заголовок".into()),
                    ("description", "Текст".into()),
                    ("host", "example.com".into()),
                    ("isSensitive", true.into()),
                    ("image", image),
                ])),
                Any(map(&[("_type", "SHARE".into())])),
            ]),
            ..Message::default()
        };
        assert_eq!(
            attachments(&m),
            [
                Attachment::Share {
                    url: "https://example.com/a".into(),
                    title: "Заголовок".into(),
                    description: "Текст".into(),
                    host: "example.com".into(),
                    image: Some("https://i.example/p".into()),
                    sensitive: true,
                },
                Attachment::Other {
                    kind: "SHARE".into()
                },
            ]
        );
    }

    #[test]
    fn reads_video_previews() {
        let m = Message {
            attaches: Some(vec![
                Any(map(&[
                    ("_type", "VIDEO".into()),
                    ("videoId", 77.into()),
                    ("thumbnail", "https://i.example/t".into()),
                    ("width", 1280.into()),
                    ("height", 720.into()),
                    ("duration", 42_000.into()),
                ])),
                Any(map(&[("_type", "VIDEO".into()), ("thumbnail", "".into())])),
            ]),
            ..Message::default()
        };
        assert_eq!(
            attachments(&m),
            [
                Attachment::Video {
                    video_id: 77,
                    thumbnail: Some("https://i.example/t".into()),
                    width: 1280,
                    height: 720,
                    duration: 42_000
                },
                Attachment::Video {
                    video_id: 0,
                    thumbnail: None,
                    width: 0,
                    height: 0,
                    duration: 0
                },
            ]
        );
    }

    #[test]
    fn photo_token_from_answer() {
        let body = serde_json::json!({"photos": {"123": {"token": "tok"}}});
        assert_eq!(photo_token(&body).unwrap(), "tok");
        assert!(photo_token(&serde_json::json!({"error_field": "x"})).is_err());
    }

    #[test]
    fn outgoing_attachment_shapes() {
        let photo = OutgoingAttachment::Photo { token: "t".into() }.to_value();
        assert_eq!(
            field(&photo, "_type").and_then(Value::as_str),
            Some("PHOTO")
        );
        assert_eq!(
            field(&photo, "photoToken").and_then(Value::as_str),
            Some("t")
        );
        let file = OutgoingAttachment::File { file_id: 7 }.to_value();
        assert_eq!(field(&file, "fileId").and_then(Value::as_i64), Some(7));
    }

    #[test]
    fn names_and_types() {
        assert!(is_photo_name("IMG_1.JPG") && !is_photo_name("doc.pdf"));
        assert_eq!(photo_mime("a.png"), "image/png");
        assert_eq!(
            urlencoding("отчёт 1.pdf"),
            "%D0%BE%D1%82%D1%87%D1%91%D1%82%201.pdf"
        );
    }
}
