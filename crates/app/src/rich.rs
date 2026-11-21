//! Rich message text: the server's formatting elements turned into byte ranges of the shown
//! text, web addresses found in plain text, and the non-overlapping style runs GPUI draws.
//!
//! Element types are the `core.dll` strings (`STRONG`, `EMPHASIZED`, `UNDERLINE`,
//! `STRIKETHROUGH`, `MONOSPACED`, `CODE`, `HEADING`, `QUOTE`, `USER_MENTION`, `LINK` with
//! `attributes.url`, `ANIMOJI` with `attributes.animojiLottieUrl`); `from` and `length` count
//! UTF-16 code units, as PyMax builds them.

use std::ops::Range;

/// What a marked range of text is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkKind {
    Bold,
    Italic,
    Underline,
    Strike,
    /// Monospaced or code.
    Mono,
    Heading,
    Quote,
    /// A mention of a user.
    Mention,
    /// A link to `url`; `hidden` when the text shown is not the address itself.
    Link {
        url: String,
        hidden: bool,
    },
    /// An animated emoji. `url` is its Lottie file (`attributes.animojiLottieUrl`).
    Animoji {
        url: String,
    },
}

/// A marked byte range of the shown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub range: Range<usize>,
    pub kind: MarkKind,
}

/// One formatting element as the server sends it.
#[derive(Debug, Clone, Copy)]
pub struct Element<'a> {
    pub kind: &'a str,
    pub from: i64,
    pub length: i64,
    pub url: Option<&'a str>,
}

/// The byte offset of UTF-16 code unit `unit` in `text`, moved forward to a character boundary
/// and clamped to the end.
pub fn utf16_to_byte(text: &str, unit: i64) -> usize {
    if unit <= 0 {
        return 0;
    }
    let mut units = 0i64;
    for (i, c) in text.char_indices() {
        if units >= unit {
            return i;
        }
        units += c.len_utf16() as i64;
    }
    text.len()
}

/// The address as people compare it: no scheme, no `www.`, no trailing slash, lower case.
fn bare(url: &str) -> String {
    let lower = url.trim().to_lowercase();
    let no_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    no_scheme
        .strip_prefix("www.")
        .unwrap_or(no_scheme)
        .trim_end_matches('/')
        .to_string()
}

/// The link text says something other than where it leads.
pub fn is_hidden(label: &str, url: &str) -> bool {
    bare(label) != bare(url)
}

/// The label can stand in for a missing link address: it names a host or already has a scheme.
fn looks_like_address(text: &str) -> bool {
    let text = text.trim();
    text.contains("://") || text.to_ascii_lowercase().starts_with("mailto:") || text.contains('.')
}

/// `https://user@host` or `https://user:pass@host`. The host is easy to misread, so a click asks
/// first. Mail addresses are not this form.
pub fn has_userinfo(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let Some((user, host)) = authority.rsplit_once('@') else {
        return false;
    };
    !user.is_empty() && !host.is_empty()
}

/// Marks from the server's elements over `text` (the message text as sent, before anything
/// is put in front of it). Elements outside the text or of other types are dropped.
pub fn from_elements<'a>(text: &str, elements: impl IntoIterator<Item = Element<'a>>) -> Vec<Mark> {
    let mut marks = Vec::new();
    for e in elements {
        let start = utf16_to_byte(text, e.from);
        let end = utf16_to_byte(text, e.from.saturating_add(e.length.max(0)));
        if start >= end {
            continue;
        }
        let kind = match e.kind {
            "STRONG" => MarkKind::Bold,
            "EMPHASIZED" => MarkKind::Italic,
            "UNDERLINE" => MarkKind::Underline,
            "STRIKETHROUGH" => MarkKind::Strike,
            "MONOSPACED" | "CODE" => MarkKind::Mono,
            "HEADING" => MarkKind::Heading,
            "QUOTE" => MarkKind::Quote,
            "USER_MENTION" => MarkKind::Mention,
            "ANIMOJI" => {
                let Some(url) = e.url.map(str::trim).filter(|u| !u.is_empty()) else {
                    continue;
                };
                MarkKind::Animoji {
                    url: url.to_string(),
                }
            }
            "LINK" => {
                let label = text[start..end].trim();
                let Some(url) = e
                    .url
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .map(str::to_string)
                    .or_else(|| looks_like_address(label).then(|| with_scheme(label)))
                else {
                    continue;
                };
                MarkKind::Link {
                    hidden: is_hidden(label, &url),
                    url,
                }
            }
            _ => continue,
        };
        marks.push(Mark {
            range: start..end,
            kind,
        });
    }
    marks
}

/// `https://` in front of an address written without a scheme.
fn with_scheme(address: &str) -> String {
    if address.contains("://") || address.starts_with("mailto:") {
        address.to_string()
    } else {
        format!("https://{address}")
    }
}

/// Web addresses written in the text (`http://`, `https://`, `www.`) that no link mark covers
/// yet, as links. Trailing punctuation and an unmatched closing bracket stay outside.
pub fn autolinks(text: &str, marks: &[Mark]) -> Vec<Mark> {
    let taken = |r: &Range<usize>| {
        marks.iter().any(|m| {
            matches!(m.kind, MarkKind::Link { .. })
                && m.range.start < r.end
                && r.start < m.range.end
        })
    };
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let next = ["https://", "http://", "www."]
            .iter()
            .filter_map(|p| lower[at..].find(p).map(|i| at + i))
            .min();
        let Some(start) = next else { break };
        // At the start of a word: not inside a host, and not the `www` of an email.
        let word_start = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '/' && c != '.' && c != '@');
        let end = text[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '«' | '»'))
            .map_or(text.len(), |i| start + i);
        let mut end = end;
        // Drop trailing punctuation, and a ")" the address did not open.
        while let Some(last) = text[start..end].chars().next_back() {
            let unmatched_paren = last == ')'
                && text[start..end].matches('(').count() < text[start..end].matches(')').count();
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '…') || unmatched_paren {
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        let range = start..end;
        let body = &text[range.clone()];
        let after_scheme = body.rsplit("://").next().unwrap_or(body);
        let host = after_scheme
            .trim_start_matches("www.")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        let has_host = host.contains('.') || host.eq_ignore_ascii_case("localhost");
        if word_start && end > start + 4 && has_host && !taken(&range) {
            out.push(Mark {
                range: range.clone(),
                kind: MarkKind::Link {
                    url: with_scheme(body),
                    hidden: false,
                },
            });
        }
        at = end.max(start + 1);
        while !text.is_char_boundary(at) {
            at += 1;
        }
    }
    out
}

/// Marks moved `by` bytes to the right (text was put in front of theirs).
pub fn shift(marks: Vec<Mark>, by: usize) -> Vec<Mark> {
    marks
        .into_iter()
        .map(|m| Mark {
            range: m.range.start + by..m.range.end + by,
            kind: m.kind,
        })
        .collect()
}

/// The styles that apply to one run of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub quote: bool,
    pub mention: bool,
    pub link: bool,
}

/// Sorted, non-overlapping runs of `len` bytes with the styles of every mark over them; runs
/// with no style are left out.
pub fn runs(len: usize, marks: &[Mark]) -> Vec<(Range<usize>, Flags)> {
    let mut cuts: Vec<usize> = vec![0, len];
    for m in marks {
        cuts.push(m.range.start.min(len));
        cuts.push(m.range.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let mut f = Flags::default();
        for m in marks
            .iter()
            .filter(|m| m.range.start <= a && b <= m.range.end)
        {
            match m.kind {
                MarkKind::Bold | MarkKind::Heading => f.bold = true,
                MarkKind::Italic => f.italic = true,
                MarkKind::Underline => f.underline = true,
                MarkKind::Strike => f.strike = true,
                MarkKind::Mono => f.mono = true,
                MarkKind::Quote => f.quote = true,
                MarkKind::Mention => f.mention = true,
                MarkKind::Link { .. } => f.link = true,
                MarkKind::Animoji { .. } => {}
            }
        }
        if f != Flags::default() {
            out.push((a..b, f));
        }
    }
    out
}

/// A slice of the message: plain (possibly formatted) text, or one animated emoji.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Bytes of the message text.
    Text(Range<usize>),
    /// An animated emoji covering those bytes.
    Animoji { range: Range<usize>, url: String },
}

/// The message split around its animated emoji, in order. Text with no animoji is one piece.
pub fn pieces(len: usize, marks: &[Mark]) -> Vec<Piece> {
    let mut anim: Vec<&Mark> = marks
        .iter()
        .filter(|m| matches!(m.kind, MarkKind::Animoji { .. }))
        .filter(|m| m.range.start < m.range.end && m.range.start < len)
        .collect();
    anim.sort_by_key(|m| m.range.start);
    let mut out = Vec::new();
    let mut at = 0;
    for m in anim {
        let start = m.range.start.max(at).min(len);
        let end = m.range.end.min(len);
        if start > at {
            out.push(Piece::Text(at..start));
        }
        if let MarkKind::Animoji { url } = &m.kind
            && end > start
        {
            out.push(Piece::Animoji {
                range: start..end,
                url: url.clone(),
            });
            at = end;
        }
    }
    if at < len {
        out.push(Piece::Text(at..len));
    }
    out
}

/// Marks that cover `range`, shifted so `range.start` is byte 0. Animated emoji are left out:
/// they are drawn as their own piece.
pub fn marks_in(marks: &[Mark], range: Range<usize>) -> Vec<Mark> {
    marks
        .iter()
        .filter_map(|m| {
            if matches!(m.kind, MarkKind::Animoji { .. }) {
                return None;
            }
            let start = m.range.start.max(range.start);
            let end = m.range.end.min(range.end);
            (start < end).then(|| Mark {
                range: start - range.start..end - range.start,
                kind: m.kind.clone(),
            })
        })
        .collect()
}

/// A link to draw as clickable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub range: Range<usize>,
    pub url: String,
    pub hidden: bool,
}

/// The links among `marks`, in text order, without overlaps.
pub fn links(marks: &[Mark]) -> Vec<Link> {
    let mut out: Vec<Link> = marks
        .iter()
        .filter_map(|m| match &m.kind {
            MarkKind::Link { url, hidden } => Some(Link {
                range: m.range.clone(),
                url: url.clone(),
                hidden: *hidden,
            }),
            _ => None,
        })
        .collect();
    out.sort_by_key(|l| l.range.start);
    let mut end = 0;
    out.retain(|l| {
        let keep = l.range.start >= end;
        if keep {
            end = l.range.end;
        }
        keep
    });
    out
}

/// Addresses the client hands to the system: web pages and mail only, with no blank or control
/// character in them.
pub fn opens_externally(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

/// Open now. A hidden address, or one with a login in front of the host, is asked about first.
pub fn opens_directly(url: &str, hidden: bool) -> bool {
    opens_externally(url) && !hidden && !has_userinfo(url)
}

/// The host of a web address: no scheme, no login, no port, no path.
pub fn host_of(url: &str) -> String {
    let trimmed = url.trim();
    let rest = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_string()
}

fn bare_host(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// `shown` is the site the card names, and it is the host of `url`.
pub fn same_host(shown: &str, url: &str) -> bool {
    let shown = bare_host(shown);
    !shown.is_empty() && shown == bare_host(&host_of(url))
}

/// A link the desktop client asks `LINK_INFO` about: it points at max.ru.
pub fn is_max_link(url: &str) -> bool {
    let host = bare_host(&host_of(url));
    host == "max.ru"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el<'a>(kind: &'a str, from: i64, length: i64, url: Option<&'a str>) -> Element<'a> {
        Element {
            kind,
            from,
            length,
            url,
        }
    }

    #[test]
    fn utf16_offsets_become_byte_ranges() {
        // "Ж" is 2 bytes, one UTF-16 unit; "👍" is 4 bytes, two units.
        let text = "Ж👍 жирно";
        assert_eq!(utf16_to_byte(text, 0), 0);
        assert_eq!(utf16_to_byte(text, 1), 2);
        assert_eq!(utf16_to_byte(text, 3), 6);
        assert_eq!(
            utf16_to_byte(text, 2),
            6,
            "inside a pair: forward to the boundary"
        );
        assert_eq!(utf16_to_byte(text, 99), text.len());
        let marks = from_elements(text, [el("STRONG", 4, 5, None)]);
        assert_eq!(&text[marks[0].range.clone()], "жирно");
    }

    #[test]
    fn kinds_links_and_dropped_elements() {
        let text = "Новости тут и max.ru";
        let marks = from_elements(
            text,
            [
                el("LINK", 8, 3, Some("https://example.com/a")),
                el("LINK", 14, 6, None),
                el("LINK", 0, 7, None),
                el("ANIMOJI", 0, 2, None),
                el("STRONG", 50, 3, None),
                el("EMPHASIZED", 0, 7, None),
            ],
        );
        assert_eq!(
            marks.len(),
            3,
            "animoji, a link that is not an address, and out-of-range elements are dropped"
        );
        assert_eq!(
            marks[0].kind,
            MarkKind::Link {
                url: "https://example.com/a".into(),
                hidden: true
            }
        );
        assert_eq!(
            marks[1].kind,
            MarkKind::Link {
                url: "https://max.ru".into(),
                hidden: false
            },
            "a link without a url leads to its own text"
        );
        assert_eq!(marks[2].kind, MarkKind::Italic);
        assert!(!is_hidden("www.Example.com/", "https://example.com"));
        let wave = "До завтра! 👋";
        let start = wave.find('👋').unwrap_or(0);
        let marks = from_elements(
            wave,
            [Element {
                kind: "ANIMOJI",
                from: 11,
                length: 2,
                url: Some("https://i.example/a.json"),
            }],
        );
        assert!(matches!(
            &marks[0].kind,
            MarkKind::Animoji { url }
                if url == "https://i.example/a.json" && marks[0].range == (start..wave.len())
        ));
        assert!(matches!(
            pieces(wave.len(), &marks).as_slice(),
            [Piece::Text(_), Piece::Animoji { .. }]
        ));
    }

    #[test]
    fn plain_addresses_become_links() {
        let text = "Смотри https://example.com/a(b), и www.max.ru. А тут http: нет; mail@x.ru нет.";
        let found = autolinks(text, &[]);
        let got: Vec<(&str, String)> = found
            .iter()
            .map(|m| match &m.kind {
                MarkKind::Link { url, .. } => (&text[m.range.clone()], url.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    "https://example.com/a(b)",
                    "https://example.com/a(b)".to_string()
                ),
                ("www.max.ru", "https://www.max.ru".to_string()),
            ]
        );
        let bracketed = "(см. https://example.com)";
        assert_eq!(
            &bracketed[autolinks(bracketed, &[])[0].range.clone()],
            "https://example.com",
            "an unmatched closing bracket stays outside"
        );
        let linked = from_elements("https://example.com", [el("LINK", 0, 19, None)]);
        assert!(
            autolinks("https://example.com", &linked).is_empty(),
            "already a link"
        );
        assert!(
            autolinks("abchttps://example.com", &[]).is_empty(),
            "inside a word"
        );
        assert!(
            autolinks("пишите a@www.max.ru пожалуйста", &[]).is_empty(),
            "the www of an email is not a link"
        );
        let local = "см. https://localhost/a";
        assert_eq!(
            &local[autolinks(local, &[])[0].range.clone()],
            "https://localhost/a"
        );
        assert!(
            autolinks("https://notahost", &[]).is_empty(),
            "a host needs a dot"
        );
    }

    #[test]
    fn overlapping_marks_split_into_runs() {
        let marks = vec![
            Mark {
                range: 0..6,
                kind: MarkKind::Bold,
            },
            Mark {
                range: 3..9,
                kind: MarkKind::Italic,
            },
        ];
        let r = runs(12, &marks);
        let bold = Flags {
            bold: true,
            ..Flags::default()
        };
        let both = Flags {
            italic: true,
            ..bold
        };
        let italic = Flags {
            italic: true,
            ..Flags::default()
        };
        assert_eq!(r, vec![(0..3, bold), (3..6, both), (6..9, italic)]);
        assert_eq!(
            shift(marks, 4)[1].range,
            7..13,
            "text put in front moves the marks"
        );
    }

    #[test]
    fn links_in_order_and_safe_schemes() {
        let link = |r: Range<usize>, u: &str| Mark {
            range: r,
            kind: MarkKind::Link {
                url: u.into(),
                hidden: false,
            },
        };
        let l = links(&[
            link(10..20, "https://b"),
            link(0..5, "https://a"),
            link(3..8, "https://c"),
        ]);
        let urls: Vec<&str> = l.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://a", "https://b"], "overlaps dropped");
        assert!(opens_externally("HTTPS://example.com"));
        assert!(opens_externally("mailto:a@b.ru"));
        assert!(!opens_externally("file:///C:/Windows"));
        assert!(!opens_externally("javascript:alert(1)"));
        assert!(!opens_externally("https://example.com/a b"));
        assert!(!opens_externally("https://example.com\n.evil"));
        assert!(has_userinfo("https://user:pass@example.com/a"));
        assert!(has_userinfo("HTTPS://example.com@evil.com"));
        assert!(!has_userinfo("mailto:a@b.ru"));
        assert!(!has_userinfo("https://example.com/a@b"));
        assert!(opens_directly("https://example.com", false));
        assert!(!opens_directly("https://example.com", true));
        assert!(!opens_directly("https://user@evil.com", false));
        assert_eq!(
            host_of("https://user:pass@www.Example.com:443/a"),
            "www.Example.com"
        );
        assert!(same_host("Example.com", "https://www.example.com/a"));
        assert!(!same_host("example.com", "https://evil.com"));
        assert!(is_max_link("https://max.ru/join/abc"));
        assert!(is_max_link("HTTPS://www.max.ru/u/1"));
        assert!(!is_max_link("https://example.com"));
        assert!(!is_max_link("https://notmax.ru/a"));
    }
}
