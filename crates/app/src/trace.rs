//! Local protocol trace (`maxrs --trace`): one line per frame and per notable step, with
//! personal data removed (`max_proto::redact`). Written only to the local data folder; nothing
//! is sent anywhere.

use std::fs::{File, OpenOptions};
use std::io::{LineWriter, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use max_proto::Opcode;
use max_proto::payload::decode_value;
use max_proto::redact::redacted;
use max_transport::{Trace, TraceFrame};

struct TraceLog {
    file: Mutex<LineWriter<File>>,
    start: Instant,
}

static LOG: OnceLock<TraceLog> = OnceLock::new();

/// Where traces go: `%LOCALAPPDATA%\maxrs\traces`.
fn traces_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("maxrs").join("traces"))
}

/// Start tracing into a new file; returns its path.
pub fn enable() -> std::io::Result<PathBuf> {
    let dir = traces_dir().ok_or_else(|| std::io::Error::other("no local data folder"))?;
    std::fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("trace-{stamp}.log"));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)?;
    let _ = LOG.set(TraceLog {
        file: Mutex::new(LineWriter::new(file)),
        start: Instant::now(),
    });
    note("trace started");
    Ok(path)
}

fn write_line(line: &str) {
    if let Some(log) = LOG.get() {
        let secs = log.start.elapsed().as_secs_f64();
        let mut file = log.file.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = writeln!(file, "{secs:9.3}  {line}");
    }
}

/// Record a step that is not a frame (HTTP results, failures). URLs are cut to their host.
pub fn note(text: &str) {
    if LOG.get().is_some() {
        write_line(&format!("--  {}", strip_urls(text)));
    }
}

/// The transport hook, when tracing is on.
pub fn transport_trace() -> Option<Trace> {
    LOG.get()?;
    Some(Trace::new(|f| write_line(&frame_line(f))))
}

fn frame_line(f: &TraceFrame<'_>) -> String {
    let h = f.header;
    let arrow = if f.sent { "->" } else { "<-" };
    let name = Opcode::from_u16(h.opcode)
        .map(|o| format!("{o:?}"))
        .unwrap_or_else(|| "?".into());
    let body = match decode_value(f.payload) {
        Ok(v) => redacted(&v),
        Err(e) => format!("<undecodable {} bytes: {e}>", f.payload.len()),
    };
    format!(
        "{arrow}  {:?} seq={} {name}({}) {body}",
        h.cmd(),
        h.seq,
        h.opcode
    )
}

/// Replace every `http(s)://host/rest` with `http(s)://host/…`.
fn strip_urls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        let (before, tail) = rest.split_at(at);
        out.push_str(before);
        let scheme = ["https://", "http://"]
            .into_iter()
            .find(|s| tail.starts_with(s));
        let Some(scheme) = scheme else {
            out.push_str("http");
            rest = &tail[4..];
            continue;
        };
        let after = &tail[scheme.len()..];
        let host_end = after.find(['/', '?', '#', ' ', ')']).unwrap_or(after.len());
        out.push_str(scheme);
        out.push_str(&after[..host_end]);
        let url_end = after.find([' ', ')']).unwrap_or(after.len());
        if url_end > host_end {
            out.push_str("/…");
        }
        rest = &after[url_end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::strip_urls;

    #[test]
    fn urls_keep_only_the_host() {
        assert_eq!(
            strip_urls("error sending request for url (https://fu.oneme.ru/api/upload.do?sig=x)"),
            "error sending request for url (https://fu.oneme.ru/…)"
        );
        assert_eq!(
            strip_urls("http status 403 from http://a.b"),
            "http status 403 from http://a.b"
        );
        assert_eq!(strip_urls("no urls, just httpd"), "no urls, just httpd");
    }
}
