//! Anchor derivation, strength classification, char-grams and presence checks.

use std::collections::BTreeSet;

use crate::config;
use crate::json::Json;
use crate::mem::Card;
use crate::normalize::{contains_cjk, is_upper, normalize_text, py_len, py_strip};
use crate::pyregex;

const TRIM_PUNCT: &[char] = &[
    '.', ',', ';', ':', '!', '?', '，', '。', '；', '：', '！', '？', '(', ')', '[', ']', '{', '}',
];

#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    pub value: String,
    pub norm: String,
    pub kind: String,
    pub strong: bool,
    pub manual: bool,
}

impl Anchor {
    pub fn strength(&self) -> &'static str {
        if self.strong {
            "strong"
        } else {
            "weak"
        }
    }
}

fn normalized_set(values: &[String]) -> BTreeSet<String> {
    values.iter().map(|v| normalize_text(v)).collect()
}

fn anchor_strength(
    value: &str,
    manual: bool,
    weak: &BTreeSet<String>,
    generic: &BTreeSet<String>,
) -> bool {
    let norm = normalize_text(value);
    let norm = norm.trim_matches(TRIM_PUNCT).to_string();
    if weak.contains(&norm) || generic.contains(&norm) {
        return false;
    }
    if pyregex::is_url_full(value) || pyregex::is_domain_full(value) || pyregex::is_path_full(value)
    {
        return true;
    }
    if manual {
        return true;
    }
    if pyregex::is_caps_identifier(value) {
        return true;
    }
    // any(ch.isupper() for ch in value[1:]) or any(ch.isdigit() for ch in value)
    let has_upper_after_first = {
        let mut rest = value.chars();
        rest.next();
        rest.any(is_upper)
    };
    has_upper_after_first || value.chars().any(|ch| ch.is_numeric())
}

fn slice_of(text: &str, span: (usize, usize)) -> &str {
    &text[span.0..span.1]
}

/// derive_anchors: deterministic manual + automatic atomic anchors.
pub fn derive_anchors(card: &Card, cfg: &Json) -> Vec<Anchor> {
    let radar = config::section(cfg, "radar");
    let search = config::section(cfg, "search");
    let stop = normalized_set(&config::string_list(config::get(radar, "stoplist")));
    let weak = normalized_set(&config::string_list(config::get(search, "weak_anchors")));
    let generic = normalized_set(&config::string_list(config::get(search, "generic_terms")));
    let ascii_min = config::get(radar, "ascii_min")
        .and_then(config::py_int)
        .unwrap_or(4)
        .max(0) as usize;
    let cjk_min = config::get(radar, "cjk_min")
        .and_then(config::py_int)
        .unwrap_or(2)
        .max(0) as usize;

    let mut found: Vec<(String, Anchor)> = Vec::new();

    fn priority(anchor: &Anchor) -> (bool, bool) {
        (anchor.strong, anchor.manual)
    }

    let add = |value: &str, kind: &str, manual: bool, found: &mut Vec<(String, Anchor)>| {
        let trimmed = py_strip(value).trim_matches(TRIM_PUNCT);
        let norm = normalize_text(trimmed);
        if norm.is_empty() || stop.contains(&norm) {
            return;
        }
        if contains_cjk(trimmed) {
            if py_len(trimmed) < cjk_min {
                return;
            }
        } else if py_len(trimmed) < ascii_min && !pyregex::is_caps_acronym(trimmed) {
            return;
        }
        let item = Anchor {
            value: trimmed.to_string(),
            norm: norm.clone(),
            kind: kind.to_string(),
            strong: anchor_strength(trimmed, manual, &weak, &generic),
            manual,
        };
        match found.iter_mut().find(|(key, _)| *key == norm) {
            None => found.push((norm, item)),
            Some((_, old)) => {
                if priority(&item) > priority(old) {
                    *old = item;
                }
            }
        }
    };

    for entity in &card.entities {
        add(entity, "manual", true, &mut found);
    }
    let mut text = card.entities.join("\n");
    text.push('\n');
    text.push_str(&card.title);
    for line in &card.body {
        text.push('\n');
        text.push_str(line);
    }
    for span in pyregex::find_urls(&text) {
        let value = slice_of(&text, span).trim_end_matches(['.', ',', ';', '，', '。', '；']);
        add(value, "url", false, &mut found);
    }
    for span in pyregex::find_domains(&text) {
        add(slice_of(&text, span), "domain", false, &mut found);
    }
    for span in pyregex::find_paths(&text) {
        let value = slice_of(&text, span);
        if py_len(value) >= 4 {
            add(value, "path", false, &mut found);
        }
    }
    for span in pyregex::find_code_tokens(&text) {
        let value = slice_of(&text, span);
        let mut rest = value.chars();
        rest.next();
        let eligible = pyregex::is_caps_identifier(value)
            || rest.clone().any(is_upper)
            || value.chars().any(|ch| ch.is_numeric())
            || value.contains('.')
            || value.contains('_')
            || value.contains('-');
        if eligible {
            add(value, "identifier", false, &mut found);
        }
    }
    let mut anchors: Vec<Anchor> = found.into_iter().map(|(_, anchor)| anchor).collect();
    anchors.sort_by(|a, b| a.norm.cmp(&b.norm).then(a.kind.cmp(&b.kind)));
    anchors
}

/// char_grams: normalize, remove all whitespace, sliding 3-grams.
pub fn char_grams(text: &str) -> BTreeSet<String> {
    let compact: String = normalize_text(text)
        .chars()
        .filter(|ch| !crate::normalize::is_py_space(*ch))
        .collect();
    let chars: Vec<char> = compact.chars().collect();
    let mut out = BTreeSet::new();
    if chars.len() < 3 {
        if !chars.is_empty() {
            out.insert(compact.clone());
        }
        return out;
    }
    for window in chars.windows(3) {
        out.insert(window.iter().collect::<String>());
    }
    out
}

/// _anchor_present: substring for CJK/punctuated anchors, ASCII boundaries else.
pub fn anchor_present(anchor: &str, text: &str) -> bool {
    if contains_cjk(anchor) || anchor.chars().any(|ch| "/._:-".contains(ch)) {
        return text.contains(anchor);
    }
    pyregex::ascii_anchor_present(anchor, text)
}
