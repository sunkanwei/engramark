//! .mem v1 cards: parse, canonical serialization, structured content rules.
//! Canonical output is byte-identical to the Python reference: UTF-8, no BOM,
//! LF endings, single trailing LF, fixed directive order.

use crate::clock::clock;
use crate::freshness_table::FRESHNESS_TEXT;
use crate::normalize::{normalize_text, py_len, py_strip};
use crate::{
    trust_text, Error, Result, MAX_CARD_BYTES, MAX_ENTITIES, MAX_ENTITY_CHARS, MAX_PUBLIC_ID,
    MAX_TITLE_CHARS, MEM_FORMAT_VERSION,
};

pub const VALID_TYPES: [&str; 3] = ["fact", "decision", "skill"];
pub const VALID_STATUS: [&str; 4] = ["candidate", "published", "archived", "tombstone"];

#[derive(Clone, Debug, Default)]
pub struct Card {
    pub id: i64,
    pub card_type: String,
    pub status: String,
    pub importance: i64,
    /// Fixed point 0..=6 representing T0, T0.5 .. T3.
    pub trust: i64,
    pub updated: String,
    pub entities: Vec<String>,
    pub source: String,
    pub lock: bool,
    pub scope: String,
    pub last_used: String,
    pub valid_from: String,
    pub valid_to: String,
    pub supersedes: Vec<i64>,
    pub title: String,
    pub body: Vec<String>,
    pub deduplicated: bool,
    pub unchanged: bool,
}

impl Card {
    pub fn new() -> Self {
        Card {
            source: "user".into(),
            ..Card::default()
        }
    }

    pub fn freshness(&self) -> f64 {
        freshness(&self.last_used, &self.updated)
    }

    pub fn freshness_text(&self) -> String {
        freshness_text(&self.last_used, &self.updated)
    }

    pub fn full_text(&self) -> String {
        let mut out = self.title.clone();
        for line in &self.body {
            out.push('\n');
            out.push_str(line);
        }
        out
    }
}

fn days_between(today_ordinal: i64, ref_text: &str) -> i64 {
    match parse_iso_date(ref_text) {
        Some(reference) => today_ordinal - reference,
        None => 0,
    }
}

fn freshness_days(last_used: &str, fallback: &str) -> i64 {
    let reference = if last_used.is_empty() {
        fallback
    } else {
        last_used
    };
    let today = match parse_iso_date(&clock().today()) {
        Some(day) => day,
        None => return 0,
    };
    days_between(today, reference).max(0)
}

pub fn freshness(last_used: &str, fallback: &str) -> f64 {
    freshness_text(last_used, fallback)
        .parse::<f64>()
        .unwrap_or(0.0)
}

pub fn freshness_text(last_used: &str, fallback: &str) -> String {
    let days = freshness_days(last_used, fallback);
    if days as usize >= FRESHNESS_TEXT.len() {
        return "0.0".into();
    }
    FRESHNESS_TEXT[days as usize].to_string()
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 0001-01-01 (ordinal matching Python date.toordinal()).
pub fn ordinal_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = year - (if month <= 2 { 1 } else { 0 });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 305
}

/// Weekday with Monday=0..Sunday=6, matching Python date.weekday().
fn weekday(ordinal: i64) -> i64 {
    (ordinal - 1).rem_euclid(7)
}

/// ISO week count of a year (52 or 53), per ISO 8601.
fn iso_weeks_in_year(year: i64) -> i64 {
    let jan1 = weekday(ordinal_from_civil(year, 1, 1));
    if jan1 == 3 || (jan1 == 2 && is_leap_year(year)) {
        53
    } else {
        52
    }
}

fn iso_ordinal(year: i64, week: i64, weekday_of_date: i64) -> i64 {
    // Week 1 contains the year's first Thursday; weekday Monday=1..Sunday=7.
    let jan4 = ordinal_from_civil(year, 1, 4);
    let jan4_weekday = weekday(jan4) + 1;
    let week1_monday = jan4 - (jan4_weekday - 1);
    week1_monday + (week - 1) * 7 + (weekday_of_date - 1)
}

/// Strict subset of datetime.date.fromisoformat accepted formats:
/// YYYY-MM-DD, YYYYMMDD, YYYY-Www, YYYYWww, YYYY-Www-D, YYYYWwwD.
pub fn parse_iso_date(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let digits = |range: &[u8]| -> Option<i64> {
        if !range.is_empty() && range.iter().all(|b| b.is_ascii_digit()) {
            std::str::from_utf8(range).ok()?.parse().ok()
        } else {
            None
        }
    };
    let civil = |year: i64, month: i64, day: i64| -> Option<i64> {
        if !(1..=9999).contains(&year)
            || !(1..=12).contains(&month)
            || day < 1
            || day > days_in_month(year, month)
        {
            return None;
        }
        Some(ordinal_from_civil(year, month, day))
    };
    let week_date = |year: i64, week: i64, weekday: i64| -> Option<i64> {
        if !(1..=9999).contains(&year)
            || week < 1
            || week > iso_weeks_in_year(year)
            || !(1..=7).contains(&weekday)
        {
            return None;
        }
        Some(iso_ordinal(year, week, weekday))
    };
    match bytes.len() {
        7 if bytes[4] == b'W' => {
            // YYYYWww
            week_date(digits(&bytes[0..4])?, digits(&bytes[5..7])?, 1)
        }
        8 if bytes.iter().all(|b| b.is_ascii_digit()) => {
            // YYYYMMDD
            civil(
                digits(&bytes[0..4])?,
                digits(&bytes[4..6])?,
                digits(&bytes[6..8])?,
            )
        }
        8 if bytes[4] == b'W' => {
            // YYYYWwwD
            week_date(
                digits(&bytes[0..4])?,
                digits(&bytes[5..7])?,
                digits(&bytes[7..8])?,
            )
        }
        8 if bytes[4] == b'-' && bytes[5] == b'W' => {
            // YYYY-Www
            week_date(digits(&bytes[0..4])?, digits(&bytes[6..8])?, 1)
        }
        10 if bytes[4] == b'-' && bytes[7] == b'-' => {
            // YYYY-MM-DD
            civil(
                digits(&bytes[0..4])?,
                digits(&bytes[5..7])?,
                digits(&bytes[8..10])?,
            )
        }
        10 if bytes[4] == b'-' && bytes[5] == b'W' && bytes[8] == b'-' => {
            // YYYY-Www-D
            week_date(
                digits(&bytes[0..4])?,
                digits(&bytes[6..8])?,
                digits(&bytes[9..10])?,
            )
        }
        _ => None,
    }
}

/// datetime.date.today() comparison used by valid-from/valid-to: the Python
/// reference compares raw strings lexicographically, so we keep that.
pub fn date_is_current(today: &str, valid_from: &str, valid_to: &str) -> bool {
    !((!valid_from.is_empty() && valid_from > today) || (!valid_to.is_empty() && valid_to < today))
}

pub fn canonical_entities(values: &[String]) -> Vec<String> {
    let mut chosen: Vec<(String, String)> = Vec::new();
    for value in values {
        let trimmed = py_strip(value);
        if trimmed.is_empty() {
            continue;
        }
        let key = normalize_text(trimmed);
        if !chosen.iter().any(|(existing, _)| *existing == key) {
            chosen.push((key, trimmed.to_string()));
        }
    }
    chosen.sort_by(|a, b| a.0.cmp(&b.0));
    chosen.into_iter().map(|(_, value)| value).collect()
}

/// normalize_structured_content: validates and normalizes title/body/entities.
/// Returns (title, body_lines, entities, card_type).
pub fn normalize_structured_content(
    title: &str,
    body: &str,
    entities: &[String],
    card_type: &str,
) -> Result<(String, Vec<String>, Vec<String>, String)> {
    let title = py_strip(title).to_string();
    if title.is_empty() {
        return Err(Error::core("标题不能为空"));
    }
    if title.contains('\n') || title.contains('\r') || title.contains('\0') {
        return Err(Error::core("标题必须是单行文本，且不能包含 NUL 字符"));
    }
    if py_len(&title) > MAX_TITLE_CHARS {
        return Err(Error::core(format!("标题最多 {MAX_TITLE_CHARS} 个字符")));
    }
    if py_len(body) > MAX_CARD_BYTES {
        return Err(Error::core(format!("正文最多 {MAX_CARD_BYTES} 个字符")));
    }
    if body.len() > MAX_CARD_BYTES {
        return Err(Error::core(format!("正文超过 {MAX_CARD_BYTES} 字节上限")));
    }
    if body.contains('\0') {
        return Err(Error::core("正文不能包含 NUL 字符"));
    }
    let body = body.replace("\r\n", "\n");
    if body.contains('\r') {
        return Err(Error::core("正文只能使用 LF 或完整 CRLF 换行"));
    }
    let stripped = body.trim_end_matches('\n');
    let body_lines: Vec<String> = if stripped.is_empty() {
        Vec::new()
    } else {
        stripped.split('\n').map(str::to_string).collect()
    };
    if entities.len() > MAX_ENTITIES {
        return Err(Error::core(format!("实体最多 {MAX_ENTITIES} 项")));
    }
    let mut normalized_entities = Vec::new();
    for entity in entities {
        let entity = py_strip(entity);
        if entity.is_empty() {
            continue;
        }
        if py_len(entity) > MAX_ENTITY_CHARS {
            return Err(Error::core(format!(
                "单个实体最多 {MAX_ENTITY_CHARS} 个字符"
            )));
        }
        if entity.contains(',')
            || entity.contains('\n')
            || entity.contains('\r')
            || entity.contains('\0')
        {
            return Err(Error::core("实体不能包含逗号、换行或 NUL 字符"));
        }
        normalized_entities.push(entity.to_string());
    }
    if !VALID_TYPES.contains(&card_type) {
        return Err(Error::core("类型只能是 fact、decision 或 skill"));
    }
    Ok((
        title,
        body_lines,
        canonical_entities(&normalized_entities),
        card_type.to_string(),
    ))
}

pub fn stored_scope(scope: &str, project: &str) -> Result<String> {
    if scope == "global" {
        return Ok("global".into());
    }
    if scope != "project" {
        return Err(Error::core("适用范围只能是 global 或 project"));
    }
    if project.is_empty() || project == "global" {
        return Err(Error::core(
            "scope=project 需要可识别的项目目录；请在项目会话中重试，或明确使用 global",
        ));
    }
    Ok(format!("project:{project}"))
}

/// build_structured_card: validates and constructs a card with id 0.
#[allow(clippy::too_many_arguments)]
pub fn build_structured_card(
    title: &str,
    body: &str,
    entities: &[String],
    card_type: &str,
    scope: &str,
    project: &str,
    status: &str,
    source: &str,
    lock: bool,
) -> Result<Card> {
    if status != "published" && status != "candidate" {
        return Err(Error::core("结构化建卡只支持正式记忆或候选记忆"));
    }
    let (title, body_lines, normalized_entities, card_type) =
        normalize_structured_content(title, body, entities, card_type)?;
    let card = Card {
        id: 0,
        card_type,
        status: status.into(),
        importance: if status == "published" { 3 } else { 2 },
        trust: if status == "published" { 6 } else { 4 },
        updated: clock().today(),
        entities: normalized_entities,
        source: source.into(),
        lock: lock && status == "published",
        scope: stored_scope(scope, project)?,
        title,
        body: body_lines,
        ..Card::new()
    };
    if serialize_card(&card).len() > MAX_CARD_BYTES {
        return Err(Error::core(format!(
            "记忆内容超过 {MAX_CARD_BYTES} 字节上限"
        )));
    }
    Ok(card)
}

struct Header {
    id: i64,
    card_type: String,
    status: String,
    importance: i64,
    trust: i64,
    updated: String,
}

fn parse_header(line: &str) -> Option<Header> {
    // ^@(\d+)\s+(\w+)\s+(\w+)\s+I([0-3])\s+
    // T(0|0.5|1|1.5|2|2.5|3)(?:\s+F(?:0|1)(?:\.\d+)?)?\s+(\d{4}-\d{2}-\d{2})$
    let rest = line.strip_prefix('@')?;
    let mut pos = 0usize;
    let bytes = rest.as_bytes();
    let take_digits = |pos: &mut usize| -> Option<i64> {
        let start = *pos;
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        std::str::from_utf8(&bytes[start..*pos]).ok()?.parse().ok()
    };
    let take_spaces = |pos: &mut usize| -> bool {
        let start = *pos;
        while *pos < bytes.len() && is_regex_space(bytes[*pos]) {
            *pos += 1;
        }
        *pos > start
    };
    let id = take_digits(&mut pos)?;
    if !take_spaces(&mut pos) {
        return None;
    }
    let word = |pos: &mut usize| -> Option<String> {
        let start = *pos;
        while *pos < bytes.len() && is_regex_word(bytes[*pos]) {
            *pos += 1;
        }
        if *pos == start {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes[start..*pos]).into_owned())
    };
    let card_type = word(&mut pos)?;
    if !take_spaces(&mut pos) {
        return None;
    }
    let status = word(&mut pos)?;
    if !take_spaces(&mut pos) {
        return None;
    }
    if bytes.get(pos) != Some(&b'I') {
        return None;
    }
    pos += 1;
    let importance = take_digits(&mut pos)?;
    if importance > 3 {
        return None;
    }
    if !take_spaces(&mut pos) {
        return None;
    }
    if bytes.get(pos) != Some(&b'T') {
        return None;
    }
    pos += 1;
    let trust_whole = take_digits(&mut pos)?;
    let mut trust_units = trust_whole * 2;
    if bytes[pos..].starts_with(b".5") {
        trust_units += 1;
        pos += 2;
    }
    if trust_whole > 3 || !(0..=6).contains(&trust_units) {
        return None;
    }
    // Optional freshness group: \s+F(0|1)(\.\d+)? — followed by mandatory \s+.
    let mut f_consumed = false;
    let mut probe = pos;
    if take_spaces(&mut probe) && bytes.get(probe) == Some(&b'F') {
        let mut after_f = probe + 1;
        let f_digit = take_digits(&mut after_f);
        let mut f_ok = matches!(f_digit, Some(0 | 1));
        if f_ok && bytes[after_f..].starts_with(b".") {
            let mut frac = after_f + 1;
            if take_digits(&mut frac).is_some() {
                after_f = frac;
            } else {
                f_ok = false;
            }
        }
        if f_ok && take_spaces(&mut after_f) {
            pos = after_f;
            f_consumed = true;
        }
    }
    if !f_consumed && !take_spaces(&mut pos) {
        return None;
    }
    if pos >= bytes.len() {
        return None;
    }
    let date = std::str::from_utf8(&bytes[pos..]).ok()?;
    if date.len() != 10
        || !date.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        })
    {
        return None;
    }
    Some(Header {
        id,
        card_type,
        status,
        importance,
        trust: trust_units,
        updated: date.to_string(),
    })
}

fn is_regex_space(byte: u8) -> bool {
    // \s inside HEADER_RE matches ASCII whitespace on this ASCII-only line.
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_regex_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub fn parse_card(text: &str) -> Result<Card> {
    if text.starts_with('\u{feff}') {
        return Err(Error::core(".mem v1 禁止 UTF-8 BOM"));
    }
    if text.contains('\0') {
        return Err(Error::core(".mem v1 禁止 NUL 字符"));
    }
    if text.len() > MAX_CARD_BYTES {
        return Err(Error::core(format!("卡片超过 {MAX_CARD_BYTES} 字节上限")));
    }
    let text = text.replace("\r\n", "\n");
    if text.contains('\r') {
        return Err(Error::core(".mem v1 只接受 LF 或完整 CRLF 换行"));
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let Some(first) = lines.first() else {
        return Err(header_error());
    };
    let Some(header) = parse_header(first) else {
        return Err(header_error());
    };
    if header.id > MAX_PUBLIC_ID {
        return Err(Error::core(format!("卡片编号超过安全上限 {MAX_PUBLIC_ID}")));
    }
    let mut card = Card {
        id: header.id,
        card_type: header.card_type,
        status: header.status,
        importance: header.importance,
        trust: header.trust,
        updated: header.updated,
        ..Card::new()
    };
    if !VALID_TYPES.contains(&card.card_type.as_str()) {
        let mut sorted = VALID_TYPES;
        sorted.sort();
        return Err(Error::core(format!(
            "类型必须是 {} 之一，收到 {}",
            crate::normalize::py_repr_str_list(&sorted),
            crate::normalize::py_repr_str(&card.card_type)
        )));
    }
    if !VALID_STATUS.contains(&card.status.as_str()) {
        let mut sorted = VALID_STATUS;
        sorted.sort();
        return Err(Error::core(format!(
            "状态必须是 {} 之一，收到 {}",
            crate::normalize::py_repr_str_list(&sorted),
            crate::normalize::py_repr_str(&card.status)
        )));
    }
    if !(0..=6).contains(&card.trust) {
        return Err(Error::core(format!(
            "可信度 T 必须在 0–3，收到 {}",
            trust_text(card.trust)
        )));
    }
    let mut title_set = false;
    let mut entity_seen = false;
    let mut source_seen = false;
    let mut format_seen = false;
    for raw in &lines[1..] {
        if title_set {
            card.body.push((*raw).to_string());
            continue;
        }
        if py_strip(raw).is_empty() {
            return Err(Error::core("标题前不允许空行"));
        }
        if let Some(rest) = raw.strip_prefix("= ") {
            if entity_seen {
                return Err(Error::core("= 实体指令只能出现一次"));
            }
            entity_seen = true;
            let parts: Vec<String> = rest.split(',').map(str::to_string).collect();
            card.entities = canonical_entities(&parts);
        } else if let Some(rest) = raw.strip_prefix("~ ") {
            if source_seen {
                return Err(Error::core("~ 来源指令只能出现一次"));
            }
            source_seen = true;
            card.source = py_strip(rest).to_string();
            if card.source.is_empty() {
                return Err(Error::core("~ 来源不能为空"));
            }
        } else if let Some(rest) = raw.strip_prefix("# ") {
            let meta = py_strip(rest);
            if meta == "lock" {
                card.lock = true;
            } else if meta == "format 1" {
                if format_seen {
                    return Err(Error::core("# format 只能出现一次"));
                }
                format_seen = true;
            } else if let Some(value) = meta.strip_prefix("scope ") {
                card.scope = py_strip(value).to_string();
            } else if let Some(value) = meta.strip_prefix("last-used ") {
                card.last_used = py_strip(value).to_string();
            } else if let Some(value) = meta.strip_prefix("valid-from ") {
                card.valid_from = py_strip(value).to_string();
            } else if let Some(value) = meta.strip_prefix("valid-to ") {
                card.valid_to = py_strip(value).to_string();
            } else if let Some(value) = meta.strip_prefix("supersedes ") {
                card.supersedes = parse_supersedes(value)?;
            } else {
                return Err(Error::core(format!("未知 .mem v1 指令：# {meta}")));
            }
        } else if raw.starts_with('=') || raw.starts_with('~') || raw.starts_with('#') {
            return Err(Error::core(format!("非法或未知 .mem v1 指令：{raw}")));
        } else {
            card.title = (*raw).to_string();
            title_set = true;
        }
    }
    if py_strip(&card.title).is_empty() {
        return Err(Error::core(
            "缺少自足标题行（卡头与指令行之后的第一行正文）",
        ));
    }
    for (label, value) in [
        ("updated", &card.updated),
        ("last-used", &card.last_used),
        ("valid-from", &card.valid_from),
        ("valid-to", &card.valid_to),
    ] {
        if !value.is_empty() && parse_iso_date(value).is_none() {
            return Err(Error::core(format!(
                "{label} 必须是 YYYY-MM-DD，收到 {}",
                crate::normalize::py_repr_str(value)
            )));
        }
    }
    if !card.valid_from.is_empty() && !card.valid_to.is_empty() && card.valid_from > card.valid_to {
        return Err(Error::core("valid-from 不得晚于 valid-to"));
    }
    if card.supersedes.iter().any(|cid| *cid <= 0) {
        return Err(Error::core("supersedes 只能引用正整数卡片 id"));
    }
    if card.supersedes.iter().any(|cid| *cid > MAX_PUBLIC_ID) {
        return Err(Error::core(format!(
            "supersedes 引用超过安全上限 {MAX_PUBLIC_ID}"
        )));
    }
    let mut unique = card.supersedes.clone();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != card.supersedes.len() {
        return Err(Error::core("supersedes 不得包含重复 id"));
    }
    if card.id != 0 && card.supersedes.contains(&card.id) {
        return Err(Error::core("卡片不能 supersedes 自己"));
    }
    Ok(card)
}

fn parse_supersedes(text: &str) -> Result<Vec<i64>> {
    // re.findall(r"\d+", text) then int()
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos].is_ascii_digit() {
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            let value: i64 = std::str::from_utf8(&bytes[start..pos])
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(i64::MAX);
            out.push(value);
        } else {
            pos += 1;
        }
    }
    Ok(out)
}

fn header_error() -> Error {
    Error::core(
        "卡头非法。首行必须形如：@<id> <fact|decision|skill> <candidate|published|archived|tombstone> I<0-3> T<0|0.5|…|3> <YYYY-MM-DD>",
    )
}

pub fn serialize_card(card: &Card) -> String {
    let mut out = format!(
        "@{} {} {} I{} T{} F{} {}\n",
        card.id,
        card.card_type,
        card.status,
        card.importance,
        trust_text(card.trust),
        card.freshness_text(),
        card.updated
    );
    if !card.entities.is_empty() {
        out.push_str("= ");
        out.push_str(&canonical_entities(&card.entities).join(", "));
        out.push('\n');
    }
    out.push_str("~ ");
    out.push_str(&card.source);
    out.push('\n');
    out.push_str(&format!("# format {MEM_FORMAT_VERSION}\n"));
    if card.lock {
        out.push_str("# lock\n");
    }
    if !card.scope.is_empty() {
        out.push_str("# scope ");
        out.push_str(&card.scope);
        out.push('\n');
    }
    if !card.last_used.is_empty() {
        out.push_str("# last-used ");
        out.push_str(&card.last_used);
        out.push('\n');
    }
    if !card.valid_from.is_empty() {
        out.push_str("# valid-from ");
        out.push_str(&card.valid_from);
        out.push('\n');
    }
    if !card.valid_to.is_empty() {
        out.push_str("# valid-to ");
        out.push_str(&card.valid_to);
        out.push('\n');
    }
    if !card.supersedes.is_empty() {
        let mut refs = card.supersedes.clone();
        refs.sort_unstable();
        refs.dedup();
        out.push_str("# supersedes ");
        out.push_str(
            &refs
                .iter()
                .map(|cid| format!("@{cid}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('\n');
    }
    out.push_str(&card.title);
    out.push('\n');
    for line in &card.body {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// card.freshness written into headers and index lines.
pub fn card_is_current(card: &Card) -> bool {
    date_is_current(&clock().today(), &card.valid_from, &card.valid_to)
}
