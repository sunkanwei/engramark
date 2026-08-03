//! Hand-built equivalents of the Python regexes that use lookbehind/lookahead
//! or the CPython `\s`/`\w`/`\b` semantics. Each finder emulates the CPython
//! engine: leftmost scanning, greedy backtracking inside a candidate match,
//! then boundary checks; on failure the engine advances one character.

use regex::Regex;
use std::sync::LazyLock;

use crate::normalize::is_word_char;

/// CPython \s as an explicit class (includes U+001C..U+001F, unlike White_Space).
const PY_SPACE_CLASS: &str = "\\s\\x{1c}\\x{1d}\\x{1e}\\x{1f}";

fn build(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| build(&format!("(?i)https?://[^{PY_SPACE_CLASS}<>\"']+")));
static DOMAIN_CORE: LazyLock<Regex> = LazyLock::new(|| build("(?:[A-Za-z0-9-]+\\.)+[A-Za-z]{2,}"));
static PATH_CORE: LazyLock<Regex> = LazyLock::new(|| {
    build(&format!(
        "(?:~|/)(?:[^{PY_SPACE_CLASS}，。；;]+/)*[^{PY_SPACE_CLASS}，。；;]*"
    ))
});
static CODE_TOKEN_CORE: LazyLock<Regex> = LazyLock::new(|| build("[A-Za-z][A-Za-z0-9_.-]{1,63}"));
static QUERY_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    build(&format!(
        "https?://[^{PY_SPACE_CLASS}<>\"']+|(?:[A-Za-z0-9-]+\\.)+[A-Za-z]{{2,}}|[A-Za-z][A-Za-z0-9_.:/-]*|[\\u{{3400}}-\\u{{9fff}}]{{2,}}|[0-9]+(?:\\.[0-9]+)*"
    ))
});

fn char_before(text: &str, byte_pos: usize) -> Option<char> {
    text[..byte_pos].chars().next_back()
}

fn char_at(text: &str, byte_pos: usize) -> Option<char> {
    text[byte_pos..].chars().next()
}

/// Advance one char from a byte position.
fn advance(text: &str, byte_pos: usize) -> usize {
    byte_pos + char_at(text, byte_pos).map(char::len_utf8).unwrap_or(1)
}

/// Emulate finditer with a lookbehind/lookahead-checked core pattern.
/// - lookbehind: called with the char before the match start; None at start.
/// - lookahead: called with the char at the match end; None at end.
fn find_with_boundaries(
    core: &Regex,
    text: &str,
    lookbehind: impl Fn(Option<char>) -> bool,
    lookahead: impl Fn(Option<char>) -> bool,
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos <= text.len() {
        let Some(matched) = core.find_at(text, pos) else {
            break;
        };
        if matched.end() == matched.start() {
            pos = advance(text, matched.end());
            continue;
        }
        let before = char_before(text, matched.start());
        let after = char_at(text, matched.end());
        if lookbehind(before) && lookahead(after) {
            out.push((matched.start(), matched.end()));
            pos = matched.end();
        } else {
            pos = advance(text, matched.start());
        }
    }
    out
}

pub fn find_urls(text: &str) -> Vec<(usize, usize)> {
    URL_RE
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn is_url_full(value: &str) -> bool {
    URL_RE
        .find(value)
        .is_some_and(|m| m.start() == 0 && m.end() == value.len())
}

pub fn find_domains(text: &str) -> Vec<(usize, usize)> {
    // (?<![@\w])(?:[A-Za-z0-9-]+\.)+[A-Za-z]{2,}(?![\w.-])
    find_with_boundaries(
        &DOMAIN_CORE,
        text,
        |before| !matches!(before, Some(ch) if ch == '@' || is_word_char(ch)),
        |after| !matches!(after, Some(ch) if ch == '.' || ch == '-' || is_word_char(ch)),
    )
}

pub fn is_domain_full(value: &str) -> bool {
    let hits = find_domains(value);
    hits.len() == 1 && hits[0] == (0, value.len())
}

pub fn find_paths(text: &str) -> Vec<(usize, usize)> {
    // (?<!\w)(?:~|/)(?:[^\s，。；;]+/)*[^\s，。；;]*
    find_with_boundaries(
        &PATH_CORE,
        text,
        |before| !matches!(before, Some(ch) if is_word_char(ch)),
        |_| true,
    )
}

pub fn is_path_full(value: &str) -> bool {
    let hits = find_paths(value);
    hits.len() == 1 && hits[0] == (0, value.len())
}

pub fn find_code_tokens(text: &str) -> Vec<(usize, usize)> {
    // \b[A-Za-z][A-Za-z0-9_.-]{1,63}\b with CPython Unicode \b.
    find_with_boundaries(
        &CODE_TOKEN_CORE,
        text,
        |before| !matches!(before, Some(ch) if is_word_char(ch)),
        |after| !matches!(after, Some(ch) if is_word_char(ch)),
    )
}

pub fn find_query_tokens(text: &str) -> Vec<(usize, usize)> {
    QUERY_TOKEN_RE
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

/// re.search(r"(?<![A-Za-z0-9_])ANCHOR(?![A-Za-z0-9_])", text) — boolean.
pub fn ascii_anchor_present(anchor: &str, text: &str) -> bool {
    let mut start = 0usize;
    while let Some(offset) = text[start..].find(anchor) {
        let at = start + offset;
        let end = at + anchor.len();
        let before_ok =
            !matches!(char_before(text, at), Some(ch) if ch.is_ascii_alphanumeric() || ch == '_');
        let after_ok =
            !matches!(char_at(text, end), Some(ch) if ch.is_ascii_alphanumeric() || ch == '_');
        if before_ok && after_ok {
            return true;
        }
        start = advance(text, at);
    }
    false
}

/// Fullmatch [A-Z][A-Z0-9_.-]{1,15}
pub fn is_caps_identifier(value: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| build("^[A-Z][A-Z0-9_.-]{1,15}$"));
    RE.is_match(value)
}

/// Fullmatch [A-Z][A-Z0-9]{1,15}
pub fn is_caps_acronym(value: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| build("^[A-Z][A-Z0-9]{1,15}$"));
    RE.is_match(value)
}
