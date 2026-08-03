//! Unicode 16.0.0 normalization frozen against Python 3.14:
//! NFKC, then full non-Turkic casefold, then collapse Python `\s` to one space
//! and strip. Character counts are Unicode scalar values; byte counts are
//! UTF-8. Rust `str::len()` is never used where Python `len(str)` is meant.

use unicode_normalization::UnicodeNormalization;

use crate::casefold_table::CASEFOLD;

/// Python 3.x `\s` / str.strip() whitespace set (NOT Unicode White_Space:
/// U+001C..U+001F are included, matching CPython's sre and str.isspace).
pub fn is_py_space(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'..='\u{1f}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Full casefold (Unicode 16.0.0 CaseFolding.txt, C+F mappings, non-Turkic).
pub fn casefold_char(ch: char, out: &mut String) {
    let cp = ch as u32;
    if let Ok(index) = CASEFOLD.binary_search_by_key(&cp, |(code, _)| *code) {
        for mapped in CASEFOLD[index].1 {
            if mapped == 0 {
                break;
            }
            if let Some(mapped) = char::from_u32(mapped) {
                out.push(mapped);
            }
        }
    } else {
        out.push(ch);
    }
}

pub fn casefold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        casefold_char(ch, &mut out);
    }
    out
}

/// unicodedata.normalize("NFKC", text).casefold() + re.sub(r"\s+", " ").strip()
pub fn normalize_text(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    let folded = casefold(&nfkc);
    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    let mut saw_text = false;
    for ch in folded.chars() {
        if is_py_space(ch) {
            pending_space = saw_text;
            continue;
        }
        if pending_space {
            out.push(' ');
        }
        pending_space = false;
        saw_text = true;
        out.push(ch);
    }
    out
}

/// Python str.strip() with the CPython whitespace set.
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

/// Python str.rstrip().
pub fn py_rstrip(text: &str) -> &str {
    text.trim_end_matches(is_py_space)
}

pub fn contains_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| ('\u{3400}'..='\u{9fff}').contains(&ch))
}

/// Python len(str): Unicode scalar value count.
pub fn py_len(text: &str) -> usize {
    text.chars().count()
}

/// Python \w membership: underscore plus Unicode alphanumerics.
pub fn is_word_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

/// Python repr() of a string: single quotes preferred, double quotes when the
/// string contains a single quote, backslash escapes for control characters.
pub fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (0x7f..=0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (c as u32) >= 0xd800 && (c as u32) <= 0xdfff => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python repr() of a list of strings: ['a', 'b']
pub fn py_repr_str_list(items: &[&str]) -> String {
    let inner: Vec<String> = items.iter().map(|item| py_repr_str(item)).collect();
    format!("[{}]", inner.join(", "))
}

/// CPython str.isupper() for a single char (cased uppercase).
pub fn is_upper(ch: char) -> bool {
    ch.is_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_data_is_16() {
        assert_eq!(crate::casefold_table::UNICODE_DATA_VERSION, (16, 0, 0));
    }

    #[test]
    fn smoke() {
        assert_eq!(normalize_text("  Straße　Ⅷ "), "strasse viii");
        assert_eq!(normalize_text("ΣΊΣΥΦΟΣ"), "σίσυφοσ");
        assert_eq!(normalize_text("a\x1cb\u{2028}c"), "a b c");
    }
}
