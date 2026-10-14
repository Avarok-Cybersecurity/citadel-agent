//! The rules: each finds ranges of the original text and never rewrites it.

use super::{Hit, KnownValue, RedactionKind};
use regex::Regex;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("redaction patterns are fixed and valid")
}

/// `key=value` and `key: value` for secret-sounding keys; only the value is replaced.
static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r#"(?i)\b(?:password|passwd|pwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|authorization|auth|cookie|session[_-]?key|credential|otp)s?\b["']?\s*[:=]\s*["']?([^\s"',;)\]}]+)"#,
    )
});
static BEARER: LazyLock<Regex> = LazyLock::new(|| compile(r"(?i)\bbearer\s+([A-Za-z0-9._~+/=-]+)"));
/// Long runs of key/token alphabet; kept only if they mix letters and digits.
static LONG_TOKEN: LazyLock<Regex> = LazyLock::new(|| compile(r"[A-Za-z0-9+/_=-]{32,}"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+"));
static UNIX_PATH: LazyLock<Regex> =
    LazyLock::new(|| compile(r#"(?:~|\.{1,2})?/[^\s"'<>,;()\[\]{}|]+"#));
static RELATIVE_PATH: LazyLock<Regex> =
    LazyLock::new(|| compile(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)+"));
static WINDOWS_PATH: LazyLock<Regex> =
    LazyLock::new(|| compile(r#"(?:\b[A-Za-z]:[\\/]|\\\\)[^\s"'<>,;|]*"#));
static IPV4: LazyLock<Regex> = LazyLock::new(|| compile(r"[0-9]{1,3}(?:\.[0-9]{1,3}){3}"));
static IPV6_CANDIDATE: LazyLock<Regex> = LazyLock::new(|| compile(r"[0-9A-Fa-f.:]{2,}"));
static DOTTED: LazyLock<Regex> =
    LazyLock::new(|| compile(r"[A-Za-z0-9][A-Za-z0-9_-]*(?:\.[A-Za-z0-9][A-Za-z0-9_-]*)+"));
/// ASCII digits with no word boundary required, so `cid_123` and `abc123` still name 123.
static DIGITS: LazyLock<Regex> = LazyLock::new(|| compile(r"[0-9]+"));

/// Extensions that make a dotted name a file rather than a host.
const FILE_EXTENSIONS: &[&str] = &[
    "7z", "aac", "avi", "bin", "bmp", "c", "conf", "cpp", "csv", "dat", "db", "dmg", "doc", "docx",
    "exe", "flac", "gif", "gz", "h", "heic", "htm", "html", "ini", "jpeg", "jpg", "js", "json",
    "key", "log", "m4a", "md", "mkv", "mov", "mp3", "mp4", "msi", "odt", "ogg", "p12", "pages",
    "pdf", "pem", "png", "ppt", "pptx", "py", "rar", "rs", "rtf", "sh", "sqlite", "svg", "tar",
    "tgz", "tif", "tiff", "toml", "ts", "tsx", "txt", "wasm", "wav", "webm", "webp", "xls", "xlsx",
    "xml", "yaml", "yml", "zip",
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The range is not the middle of a longer word.
fn bounded(input: &str, start: usize, end: usize) -> bool {
    let before = input[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !is_word(c));
    let after = input[end..].chars().next().is_none_or(|c| !is_word(c));
    before && after
}

pub(super) fn known_value_hits(input: &str, known: &[KnownValue]) -> Vec<Hit> {
    let mut hits = Vec::new();
    if known.is_empty() {
        return hits;
    }
    // ASCII folding keeps byte offsets identical to the input's.
    let haystack = input.to_ascii_lowercase();
    for known in known {
        let needle = known.value.trim().to_ascii_lowercase();
        if needle.is_empty() {
            continue;
        }
        for (start, matched) in haystack.match_indices(needle.as_str()) {
            let end = start + matched.len();
            if bounded(input, start, end) {
                hits.push(Hit {
                    start,
                    end,
                    kind: known.kind,
                });
            }
        }
    }
    hits
}

pub(super) fn pattern_hits(input: &str, hits: &mut Vec<Hit>) {
    for regex in [&*SECRET_ASSIGNMENT, &*BEARER] {
        for caps in regex.captures_iter(input) {
            if let Some(value) = caps.get(1) {
                push(hits, value.start(), value.end(), RedactionKind::Secret);
            }
        }
    }
    for m in LONG_TOKEN.find_iter(input) {
        let text = m.as_str();
        let mixed = text.bytes().any(|b| b.is_ascii_digit())
            && text.bytes().any(|b| b.is_ascii_alphabetic());
        if mixed && bounded(input, m.start(), m.end()) {
            push(hits, m.start(), m.end(), RedactionKind::Secret);
        }
    }
    for m in EMAIL.find_iter(input) {
        push(hits, m.start(), m.end(), RedactionKind::Email);
    }
    path_hits(input, hits);
    ip_hits(input, hits);
    for m in DOTTED.find_iter(input) {
        if !bounded(input, m.start(), m.end()) {
            continue;
        }
        if let Some(kind) = classify_dotted(m.as_str()) {
            push(hits, m.start(), m.end(), kind);
        }
    }
}

fn path_hits(input: &str, hits: &mut Vec<Hit>) {
    for m in UNIX_PATH.find_iter(input) {
        let before = input[..m.start()].chars().next_back();
        let path = m.as_str().trim_end_matches(['.', ':', ',']);
        let has_name = path.chars().any(char::is_alphanumeric);
        if before.is_none_or(|c| !is_word(c)) && has_name {
            push(hits, m.start(), m.start() + path.len(), RedactionKind::Path);
        }
    }
    for m in RELATIVE_PATH.find_iter(input) {
        let path = m.as_str().trim_end_matches('.');
        let deep = path.matches('/').count() >= 2;
        let names_file = path
            .rsplit('/')
            .next()
            .is_some_and(|last| classify_dotted(last) == Some(RedactionKind::File));
        if (deep || names_file) && bounded(input, m.start(), m.start() + path.len()) {
            push(hits, m.start(), m.start() + path.len(), RedactionKind::Path);
        }
    }
    for m in WINDOWS_PATH.find_iter(input) {
        let path = m.as_str().trim_end_matches(['.', ':', ',']);
        push(hits, m.start(), m.start() + path.len(), RedactionKind::Path);
    }
}

fn ip_hits(input: &str, hits: &mut Vec<Hit>) {
    for m in IPV4.find_iter(input) {
        let before = input[..m.start()].chars().next_back();
        let mut after = input[m.end()..].chars();
        let next = after.next();
        let continues = next.is_some_and(|c| c.is_ascii_digit())
            || (next == Some('.') && after.next().is_some_and(|c| c.is_ascii_digit()));
        let standalone = before.is_none_or(|c| !c.is_ascii_digit() && c != '.') && !continues;
        if standalone && m.as_str().parse::<Ipv4Addr>().is_ok() {
            push(hits, m.start(), m.end(), RedactionKind::Ip);
        }
    }
    for m in IPV6_CANDIDATE.find_iter(input) {
        let text = m.as_str().trim_end_matches('.');
        let end = m.start() + text.len();
        if text.matches(':').count() >= 2
            && bounded(input, m.start(), end)
            && text.parse::<Ipv6Addr>().is_ok()
        {
            push(hits, m.start(), end, RedactionKind::Ip);
        }
    }
}

/// A dotted name is a file if its last label is a known extension, a host if its last
/// label is alphabetic; anything else (versions, durations, all-numeric) is left alone.
fn classify_dotted(text: &str) -> Option<RedactionKind> {
    let (stem, last) = text.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    let last_lower = last.to_ascii_lowercase();
    if FILE_EXTENSIONS.contains(&last_lower.as_str()) {
        return Some(RedactionKind::File);
    }
    (last.len() >= 2 && last.bytes().all(|b| b.is_ascii_alphabetic()))
        .then_some(RedactionKind::Host)
}

fn push(hits: &mut Vec<Hit>, start: usize, end: usize, kind: RedactionKind) {
    if start < end {
        hits.push(Hit { start, end, kind });
    }
}

/// Every standalone run of digits that fits a u64: the cids a message may name.
pub(crate) fn cid_tokens(input: &str) -> impl Iterator<Item = u64> + '_ {
    DIGITS
        .find_iter(input)
        .filter_map(|m| m.as_str().parse().ok())
}
