//! Display text operations: truncation, excerpts and human-facing lines.
//! All truncation keeps UTF-8 code points whole and matches Python's limits.

use crate::clock::clock;
use crate::config;
use crate::json::Json;
use crate::mem::freshness_text;
use crate::normalize::{is_py_space, py_len, py_rstrip, py_strip};
use crate::{
    trust_text, EXCERPT_MAX_SCAN_CODEPOINTS, HOOK_MAX_LINE_BYTES, HOOK_MAX_LINE_CODEPOINTS,
    MAX_ENTITY_CHARS, RADAR_GIST_MAX_CODEPOINTS, SEARCH_PREVIEW_MAX_BYTES,
};

pub fn unsafe_display_character(ch: char) -> bool {
    let value = ch as u32;
    value < 32 || (0x7f..=0x9f).contains(&value) || ch == '\u{2028}' || ch == '\u{2029}'
}

fn fits_text_limits(text: &str, max_codepoints: Option<usize>, max_bytes: Option<usize>) -> bool {
    max_codepoints.is_none_or(|limit| py_len(text) <= limit)
        && max_bytes.is_none_or(|limit| text.len() <= limit)
}

pub fn truncate_text(
    text: &str,
    max_codepoints: Option<usize>,
    max_bytes: Option<usize>,
    suffix: &str,
) -> String {
    if fits_text_limits(text, max_codepoints, max_bytes) {
        return text.to_string();
    }
    if suffix.is_empty() || !fits_text_limits(suffix, max_codepoints, max_bytes) {
        return String::new();
    }
    let codepoint_budget = max_codepoints.map(|limit| limit.saturating_sub(py_len(suffix)));
    let byte_budget = max_bytes.map(|limit| limit.saturating_sub(suffix.len()));
    let mut out: Vec<char> = Vec::new();
    let mut used_bytes = 0usize;
    for ch in text.chars() {
        let encoded = ch.len_utf8();
        if codepoint_budget.is_some_and(|budget| out.len() >= budget)
            || byte_budget.is_some_and(|budget| used_bytes + encoded > budget)
        {
            break;
        }
        out.push(ch);
        used_bytes += encoded;
    }
    let mut prefix: String = py_rstrip(&out.into_iter().collect::<String>()).to_string();
    while !prefix.is_empty()
        && !fits_text_limits(&format!("{prefix}{suffix}"), max_codepoints, max_bytes)
    {
        prefix = prefix
            [..prefix.len() - prefix.chars().next_back().map(char::len_utf8).unwrap_or(1)]
            .to_string();
    }
    format!("{prefix}{suffix}")
}

/// humanize_memory_text: re.sub(r"@(\d+)\b", "记忆 \1")
pub fn humanize_memory_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        if bytes[pos] == b'@' {
            let start = pos + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                // \b after digits: next char must not be a word char.
                let boundary = text[end..]
                    .chars()
                    .next()
                    .is_none_or(|ch| !crate::normalize::is_word_char(ch));
                if boundary {
                    out.push_str("记忆 ");
                    out.push_str(&text[start..end]);
                    pos = end;
                    continue;
                }
            }
        }
        let ch_len = text[pos..].chars().next().map(char::len_utf8).unwrap_or(1);
        out.push_str(&text[pos..pos + ch_len]);
        pos += ch_len;
    }
    out
}

pub fn human_display_title(text: &str, maximum: usize) -> String {
    let safe: String = text
        .chars()
        .map(|ch| {
            if unsafe_display_character(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let humanized = humanize_memory_text(&safe);
    let mut collapsed = String::with_capacity(humanized.len());
    let mut pending_space = false;
    let mut saw_text = false;
    for ch in humanized.chars() {
        if is_py_space(ch) {
            pending_space = saw_text;
            continue;
        }
        if pending_space {
            collapsed.push(' ');
        }
        pending_space = false;
        saw_text = true;
        collapsed.push(ch);
    }
    let title = collapsed;
    if py_len(&title) <= maximum {
        return title;
    }
    let truncated: String = title.chars().take(maximum - 1).collect();
    format!("{}…", py_rstrip(&truncated))
}

/// _compact_text_prefix behind memory_excerpt.
fn compact_text_prefix(
    text: &str,
    max_codepoints: Option<usize>,
    max_bytes: Option<usize>,
    first_paragraph: bool,
) -> String {
    if max_codepoints.is_some_and(|limit| limit == 0) || max_bytes.is_some_and(|limit| limit == 0) {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::new();
    let mut used_bytes = 0usize;
    let mut pending_space = false;
    let mut newline_count = 0usize;
    let mut saw_text = false;
    let mut index = 0usize;
    let mut scanned = 0usize;
    while index < chars.len() && scanned < EXCERPT_MAX_SCAN_CODEPOINTS {
        let ch = chars[index];
        if ch == '\r' {
            let consumed = if index + 1 < chars.len() && chars[index + 1] == '\n' {
                2
            } else {
                1
            };
            index += consumed;
            scanned += consumed;
            newline_count += 1;
            pending_space = saw_text;
            if first_paragraph && saw_text && newline_count >= 2 {
                break;
            }
            continue;
        }
        index += 1;
        scanned += 1;
        if is_py_space(ch) {
            if ch == '\n' {
                newline_count += 1;
                if first_paragraph && saw_text && newline_count >= 2 {
                    break;
                }
            }
            pending_space = saw_text;
            continue;
        }
        if unsafe_display_character(ch) {
            pending_space = saw_text;
            continue;
        }
        let additions: Vec<char> = if pending_space && !out.is_empty() {
            vec![' ', ch]
        } else {
            vec![ch]
        };
        newline_count = 0;
        pending_space = false;
        saw_text = true;
        for addition in additions {
            out.push(addition);
            used_bytes += addition.len_utf8();
        }
        if max_codepoints.is_some_and(|limit| out.len() > limit)
            || max_bytes.is_some_and(|limit| used_bytes > limit)
        {
            break;
        }
    }
    let compact: String = py_strip(&out.into_iter().collect::<String>()).to_string();
    truncate_text(
        &humanize_memory_text(&compact),
        max_codepoints,
        max_bytes,
        "…",
    )
}

pub fn memory_excerpt(
    text: &str,
    max_codepoints: Option<usize>,
    max_bytes: Option<usize>,
    first_paragraph: bool,
) -> String {
    compact_text_prefix(text, max_codepoints, max_bytes, first_paragraph)
}

/// Shared meta row used by search/top/radar rendering (a subset of cache meta).
#[derive(Clone, Debug, Default)]
pub struct MetaRow {
    pub id: i64,
    pub status: String,
    pub card_type: String,
    pub i: i64,
    pub t: i64,
    pub last_used: String,
    pub updated: String,
    pub source: String,
    pub lock: bool,
    pub scope: String,
    pub title: String,
    pub body: String,
    pub entities: String,
    pub valid_from: String,
    pub valid_to: String,
    pub supersedes: String,
    pub semantic_hash: String,
    pub source_hash: String,
    pub score: f64,
    pub evidence: String,
    pub strong_anchor: bool,
    pub rrf: f64,
    pub confidence: String,
}

impl MetaRow {
    pub fn freshness_text(&self) -> String {
        freshness_text(&self.last_used, &self.updated)
    }

    /// rank_key: (lock, i, t, freshness)
    pub fn rank_key(&self) -> (bool, i64, i64, f64) {
        (
            self.lock,
            self.i,
            self.t,
            freshness_text(&self.last_used, &self.updated)
                .parse::<f64>()
                .unwrap_or(0.0),
        )
    }

    pub fn is_current(&self) -> bool {
        let today = clock().today();
        !((!self.valid_from.is_empty() && self.valid_from > today)
            || (!self.valid_to.is_empty() && self.valid_to < today))
    }
}

pub fn index_line(row: &MetaRow, explain: bool) -> String {
    let f = row.freshness_text();
    let trust = trust_text(row.t);
    let prefix = if row.confidence == "medium" {
        "可能相关："
    } else {
        ""
    };
    let mut line = format!(
        "{prefix}@{} {} [{} I{} T{} F{}，详情 memory_get({})]",
        row.id, row.title, row.card_type, row.i, trust, f, row.id
    );
    if explain && !row.evidence.is_empty() {
        line.push_str(&format!("（{}；置信度 {:.2}）", row.evidence, row.score));
    }
    line
}

fn human_index_header(row: &MetaRow) -> String {
    let prefix = if row.confidence == "medium" {
        "可能相关："
    } else {
        ""
    };
    format!(
        "{prefix}记忆 {}：{}",
        row.id,
        human_display_title(&row.title, 160)
    )
}

pub fn human_index_line(row: &MetaRow) -> String {
    let mut line = human_index_header(row);
    let summary = memory_excerpt(&row.body, Some(160), None, false);
    if !summary.is_empty() {
        line.push_str(&format!("\n  摘要：{summary}"));
    }
    line
}

pub fn human_search_line(row: &MetaRow, position: usize, search_cfg: Option<&Json>) -> String {
    let (enabled, max_bytes) = match search_cfg {
        Some(cfg) => {
            let enabled = config::get(Some(cfg), "preview_enabled")
                .and_then(Json::as_bool)
                .unwrap_or(true);
            let max_bytes = config::bounded_int(
                config::get(Some(cfg), "preview_max_bytes"),
                SEARCH_PREVIEW_MAX_BYTES as i64,
                1,
                SEARCH_PREVIEW_MAX_BYTES as i64,
            ) as usize;
            (enabled, max_bytes)
        }
        None => (true, SEARCH_PREVIEW_MAX_BYTES),
    };
    if enabled && position == 0 && row.confidence == "high" && !row.body.is_empty() {
        let preview = memory_excerpt(&row.body, None, Some(max_bytes), false);
        if !preview.is_empty() {
            return format!("{}\n  正文预览：{preview}", human_index_header(row));
        }
    }
    human_index_line(row)
}

pub fn human_radar_line(row: &MetaRow, entity: &str, gist_max_codepoints: i64) -> String {
    let required = format!(
        "记忆提示：记忆 {}：{}",
        row.id,
        human_display_title(&row.title, 160)
    );
    if !fits_text_limits(
        &required,
        Some(HOOK_MAX_LINE_CODEPOINTS),
        Some(HOOK_MAX_LINE_BYTES),
    ) {
        return String::new();
    }
    let mut line = required;
    let compact_entity = if entity.is_empty() {
        String::new()
    } else {
        memory_excerpt(entity, Some(MAX_ENTITY_CHARS), None, false)
    };
    let reason = if compact_entity.is_empty() {
        String::new()
    } else {
        format!("（与“{compact_entity}”相关）")
    };
    if !reason.is_empty()
        && fits_text_limits(
            &format!("{line}{reason}"),
            Some(HOOK_MAX_LINE_CODEPOINTS),
            Some(HOOK_MAX_LINE_BYTES),
        )
    {
        line.push_str(&reason);
    }
    let gist_limit = config::bounded_int(
        Some(&Json::Int(gist_max_codepoints)),
        RADAR_GIST_MAX_CODEPOINTS as i64,
        0,
        RADAR_GIST_MAX_CODEPOINTS as i64,
    ) as usize;
    if gist_limit > 0 {
        let separator = " — ";
        let remaining_codepoints =
            HOOK_MAX_LINE_CODEPOINTS.saturating_sub(py_len(&format!("{line}{separator}")));
        let remaining_bytes =
            HOOK_MAX_LINE_BYTES.saturating_sub(format!("{line}{separator}").len());
        let gist = memory_excerpt(
            &row.body,
            Some(gist_limit.min(remaining_codepoints)),
            Some(remaining_bytes),
            true,
        );
        if !gist.is_empty() {
            line.push_str(separator);
            line.push_str(&gist);
        }
    }
    line
}

pub fn strip_title(title: &str) -> &str {
    py_strip(title)
}
