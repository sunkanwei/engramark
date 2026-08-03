//! Radar blob (MRDR v1): build, decode and validate; Aho-Corasick scanning;
//! hit extraction with scope rules. Blob bytes are identical to Python's.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use crate::anchors::{anchor_present, derive_anchors};
use crate::hash::sha256_raw;
use crate::json::Json;
use crate::mem::{card_is_current, Card};
use crate::normalize::{normalize_text, py_len};
use crate::{Error, Result, NORMALIZATION_VERSION, RADAR_COMPILER_VERSION};

#[derive(Clone, Debug, PartialEq)]
pub struct RadarCardRef {
    pub id: i64,
    pub strong: bool,
    pub kind: String,
    pub manual: bool,
    pub scope: String,
}

#[derive(Clone, Debug)]
pub struct RadarBucket {
    pub display: String,
    pub cards: Vec<RadarCardRef>,
}

pub type RadarAnchors = BTreeMap<String, RadarBucket>;

/// _radar_source: eligible published cards contribute their anchors.
pub fn radar_source(cards: &[Card], cfg: &Json) -> RadarAnchors {
    let superseded: HashSet<i64> = cards
        .iter()
        .filter(|card| card.status == "published" && card_is_current(card))
        .flat_map(|card| card.supersedes.iter().copied())
        .collect();
    let mut anchors = RadarAnchors::new();
    for card in cards {
        if card.status != "published" || card.importance < 1 || card.trust < 2 {
            continue;
        }
        if superseded.contains(&card.id)
            || card.source.starts_with("external")
            || !card_is_current(card)
        {
            continue;
        }
        for anchor in derive_anchors(card, cfg) {
            let bucket = anchors
                .entry(anchor.norm.clone())
                .or_insert_with(|| RadarBucket {
                    display: anchor.value.clone(),
                    cards: Vec::new(),
                });
            bucket.cards.push(RadarCardRef {
                id: card.id,
                strong: anchor.strong,
                kind: anchor.kind.clone(),
                manual: anchor.manual,
                scope: card.scope.clone(),
            });
        }
    }
    anchors
}

#[derive(Clone, Debug, Default)]
pub struct AhoCorasick {
    /// Insertion-ordered transitions, mirroring Python dict ordering.
    pub goto: Vec<Vec<(char, u32)>>,
    pub out: Vec<Vec<String>>,
    pub fail: Vec<u32>,
}

impl AhoCorasick {
    fn transition(&self, node: u32, ch: char) -> Option<u32> {
        self.goto[node as usize]
            .iter()
            .find(|(key, _)| *key == ch)
            .map(|(_, next)| *next)
    }

    pub fn scan(&self, text: &str, deadline: Option<Instant>) -> Result<HashSet<String>> {
        let mut node = 0u32;
        let mut found = HashSet::new();
        for (index, ch) in text.chars().enumerate() {
            if deadline.is_some_and(|d| index % 64 == 0 && Instant::now() >= d) {
                return Err(Error::HookDeadlineExceeded);
            }
            while node != 0 && self.transition(node, ch).is_none() {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Err(Error::HookDeadlineExceeded);
                }
                node = self.fail[node as usize];
            }
            node = self.transition(node, ch).unwrap_or(0);
            found.extend(self.out[node as usize].iter().cloned());
        }
        Ok(found)
    }
}

fn goto_get(goto: &[Vec<(char, u32)>], node: u32, ch: char) -> Option<u32> {
    goto[node as usize]
        .iter()
        .find(|(key, _)| *key == ch)
        .map(|(_, next)| *next)
}

/// _build_radar_blob: MRDR header + sections (meta, machine, anchors).
pub fn build_radar_blob(cards: &[Card], cfg: &Json) -> Vec<u8> {
    let anchors = radar_source(cards, cfg);
    let mut goto: Vec<Vec<(char, u32)>> = vec![Vec::new()];
    let mut out: Vec<Vec<String>> = vec![Vec::new()];
    for word in anchors.keys() {
        let mut node = 0u32;
        for ch in word.chars() {
            let next = match goto_get(&goto, node, ch) {
                Some(next) => next,
                None => {
                    let next = goto.len() as u32;
                    goto[node as usize].push((ch, next));
                    goto.push(Vec::new());
                    out.push(Vec::new());
                    next
                }
            };
            node = next;
        }
        out[node as usize].push(word.clone());
    }
    let mut fail = vec![0u32; goto.len()];
    let mut queue: Vec<u32> = goto[0].iter().map(|(_, next)| *next).collect();
    while !queue.is_empty() {
        let r = queue.remove(0);
        let transitions = goto[r as usize].clone();
        for (ch, s) in transitions {
            queue.push(s);
            let mut state = fail[r as usize];
            while state != 0 && goto_get(&goto, state, ch).is_none() {
                state = fail[state as usize];
            }
            let target = goto_get(&goto, state, ch).unwrap_or(0);
            fail[s as usize] = target;
            let inherited = out[target as usize].clone();
            out[s as usize].extend(inherited);
        }
    }
    let edge_count: usize = goto.iter().map(Vec::len).sum();
    let meta = crate::jobject! {
        "compiler_version" => RADAR_COMPILER_VERSION,
        "normalization_version" => NORMALIZATION_VERSION,
        "states" => goto.len() as i64,
        "edges" => edge_count as i64,
        "max_index" => goto.len() as i64 - 1,
    };
    let machine = machine_json(&goto, &out, &fail);
    let anchors_json = anchors_json(&anchors);
    let sections = [
        (1u16, meta.dumps_canonical().into_bytes()),
        (2u16, machine.dumps_canonical().into_bytes()),
        (3u16, anchors_json.dumps_canonical().into_bytes()),
    ];
    let mut body = Vec::new();
    for (kind, payload) in &sections {
        body.extend_from_slice(&kind.to_be_bytes());
        body.extend_from_slice(&1u16.to_be_bytes()); // flags: required
        body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        body.extend_from_slice(&sha256_raw(payload));
        body.extend_from_slice(payload);
    }
    let mut blob = Vec::with_capacity(body.len() + 12);
    blob.extend_from_slice(b"MRDR");
    blob.extend_from_slice(&1u16.to_be_bytes());
    blob.extend_from_slice(&(sections.len() as u16).to_be_bytes());
    blob.extend_from_slice(&(body.len() as u32).to_be_bytes());
    blob.extend_from_slice(&body);
    blob
}

fn machine_json(goto: &[Vec<(char, u32)>], out: &[Vec<String>], fail: &[u32]) -> Json {
    let goto_json: Vec<Json> = goto
        .iter()
        .map(|transitions| {
            Json::Object(
                transitions
                    .iter()
                    .map(|(ch, next)| (ch.to_string(), Json::Int(*next as i64)))
                    .collect(),
            )
        })
        .collect();
    let out_json: Vec<Json> = out
        .iter()
        .map(|words| Json::Array(words.iter().map(|w| Json::Str(w.clone())).collect()))
        .collect();
    let fail_json: Vec<Json> = fail.iter().map(|v| Json::Int(*v as i64)).collect();
    crate::jobject! {
        "goto" => Json::Array(goto_json),
        "out" => Json::Array(out_json),
        "fail" => Json::Array(fail_json),
    }
}

fn anchors_json(anchors: &RadarAnchors) -> Json {
    Json::Object(
        anchors
            .iter()
            .map(|(norm, bucket)| {
                let cards: Vec<Json> = bucket
                    .cards
                    .iter()
                    .map(|card| {
                        crate::jobject! {
                            "id" => card.id,
                            "strength" => if card.strong { "strong" } else { "weak" },
                            "kind" => card.kind.clone(),
                            "manual" => card.manual,
                            "scope" => card.scope.clone(),
                        }
                    })
                    .collect();
                (
                    norm.clone(),
                    crate::jobject! {
                        "display" => bucket.display.clone(),
                        "cards" => Json::Array(cards),
                    },
                )
            })
            .collect(),
    )
}

fn json_int(value: Option<&Json>) -> Option<i64> {
    value.and_then(Json::as_i64)
}

/// _decode_radar_blob with the full structural validation of the Python side.
pub fn decode_radar_blob(blob: &[u8]) -> Result<(RadarAnchors, AhoCorasick)> {
    if blob.len() < 12 || blob.len() > 32 * 1024 * 1024 {
        return Err(Error::cache("雷达缓存长度非法"));
    }
    let (magic, rest) = blob.split_at(4);
    if magic != b"MRDR" {
        return Err(Error::cache("雷达缓存头非法或版本不兼容"));
    }
    let version = u16::from_be_bytes([rest[0], rest[1]]);
    let count = u16::from_be_bytes([rest[2], rest[3]]) as usize;
    let body_len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
    if version != 1 || count > 16 || body_len != blob.len() - 12 {
        return Err(Error::cache("雷达缓存头非法或版本不兼容"));
    }
    let mut offset = 12usize;
    let mut known: HashMap<u16, &[u8]> = HashMap::new();
    for _ in 0..count {
        if offset + 40 > blob.len() {
            return Err(Error::cache("雷达区段头越界"));
        }
        let kind = u16::from_be_bytes([blob[offset], blob[offset + 1]]);
        let flags = u16::from_be_bytes([blob[offset + 2], blob[offset + 3]]);
        let length = u32::from_be_bytes([
            blob[offset + 4],
            blob[offset + 5],
            blob[offset + 6],
            blob[offset + 7],
        ]) as usize;
        let digest = &blob[offset + 8..offset + 40];
        offset += 40;
        if length > blob.len() - offset {
            return Err(Error::cache("雷达区段长度越界"));
        }
        let payload = &blob[offset..offset + length];
        offset += length;
        if sha256_raw(payload) != digest {
            return Err(Error::cache("雷达区段校验失败"));
        }
        if !(1..=3).contains(&kind) {
            if flags & 1 != 0 {
                return Err(Error::cache(format!("存在未知必需雷达区段 {kind}")));
            }
            continue;
        }
        if known.contains_key(&kind) {
            return Err(Error::cache(format!("雷达区段 {kind} 重复")));
        }
        known.insert(kind, payload);
    }
    if offset != blob.len() || !(1..=3).all(|kind| known.contains_key(&kind)) {
        return Err(Error::cache("雷达缓存缺少必需区段"));
    }
    let parse = |kind: u16| -> Result<Json> {
        let text =
            std::str::from_utf8(known[&kind]).map_err(|_| Error::cache("雷达区段 JSON 非法"))?;
        Json::parse(text).map_err(|_| Error::cache("雷达区段 JSON 非法"))
    };
    let meta = parse(1)?;
    let machine = parse(2)?;
    let anchors_raw = parse(3)?;
    if !meta.is_object() || !machine.is_object() {
        return Err(Error::cache("雷达元数据或自动机区段非法"));
    }
    let compiler_version = json_int(meta.get("compiler_version"));
    let normalization_version = json_int(meta.get("normalization_version"));
    let states = json_int(meta.get("states"));
    let edges = json_int(meta.get("edges"));
    let max_index = json_int(meta.get("max_index"));
    let (
        Some(compiler_version),
        Some(normalization_version),
        Some(states),
        Some(edges),
        Some(max_index),
    ) = (
        compiler_version,
        normalization_version,
        states,
        edges,
        max_index,
    )
    else {
        return Err(Error::cache("雷达元数据字段非法"));
    };
    if compiler_version != RADAR_COMPILER_VERSION || normalization_version != NORMALIZATION_VERSION
    {
        return Err(Error::cache("雷达编译器或规范化版本不兼容"));
    }
    let (Some(goto_json), Some(out_json), Some(fail_json)) = (
        machine.get("goto").and_then(Json::as_array),
        machine.get("out").and_then(Json::as_array),
        machine.get("fail").and_then(Json::as_array),
    ) else {
        return Err(Error::cache("雷达自动机结构非法"));
    };
    if !(0 < states && states <= 1_000_000)
        || goto_json.len() as i64 != states
        || out_json.len() as i64 != states
        || fail_json.len() as i64 != states
        || max_index != states - 1
        || !(0..=5_000_000).contains(&edges)
    {
        return Err(Error::cache("雷达自动机规模字段非法"));
    }
    let states = states as usize;
    let mut goto: Vec<Vec<(char, u32)>> = Vec::with_capacity(states);
    let mut incoming = vec![0usize; states];
    let mut actual_edges = 0usize;
    for (node, transitions) in goto_json.iter().enumerate() {
        let Some(pairs) = transitions.as_object() else {
            return Err(Error::cache(format!("雷达状态 {node} 转移非法")));
        };
        actual_edges += pairs.len();
        let mut row = Vec::with_capacity(pairs.len());
        for (ch, next) in pairs {
            let mut chars = ch.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return Err(Error::cache(format!("雷达状态 {node} 存在越界转移")));
            };
            let Some(next) = next.as_i64() else {
                return Err(Error::cache(format!("雷达状态 {node} 存在越界转移")));
            };
            if !(0..states as i64).contains(&next) {
                return Err(Error::cache(format!("雷达状态 {node} 存在越界转移")));
            }
            incoming[next as usize] += 1;
            row.push((ch, next as u32));
        }
        goto.push(row);
    }
    if actual_edges as i64 != edges {
        return Err(Error::cache("雷达边数或失败指针非法"));
    }
    let mut fail: Vec<u32> = Vec::with_capacity(states);
    for value in fail_json {
        let Some(v) = value.as_i64() else {
            return Err(Error::cache("雷达边数或失败指针非法"));
        };
        if !(0..states as i64).contains(&v) {
            return Err(Error::cache("雷达边数或失败指针非法"));
        }
        fail.push(v as u32);
    }
    if incoming[0] != 0 || incoming[1..].iter().any(|count| *count != 1) {
        return Err(Error::cache("雷达自动机不是单根前缀树"));
    }
    let mut depth = vec![-1i64; states];
    depth[0] = 0;
    let mut queue = vec![0usize];
    let mut head = 0usize;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        for (_, next) in &goto[node] {
            let next = *next as usize;
            if depth[next] != -1 {
                return Err(Error::cache("雷达自动机存在转移环"));
            }
            depth[next] = depth[node] + 1;
            queue.push(next);
        }
    }
    if queue.len() != states
        || fail[0] != 0
        || (1..states).any(|node| depth[fail[node] as usize] >= depth[node])
    {
        return Err(Error::cache("雷达自动机不可达或失败指针成环"));
    }
    let Some(anchor_pairs) = anchors_raw.as_object() else {
        return Err(Error::cache("雷达锚点区段非法"));
    };
    let allowed_kinds = ["manual", "url", "domain", "path", "identifier"];
    let mut anchors = RadarAnchors::new();
    for (anchor, bucket) in anchor_pairs {
        if anchor.is_empty() {
            return Err(Error::cache("雷达锚点结构非法"));
        }
        let (Some(display), Some(cards)) = (
            bucket.get("display").and_then(Json::as_str),
            bucket.get("cards").and_then(Json::as_array),
        ) else {
            return Err(Error::cache("雷达锚点结构非法"));
        };
        let mut refs = Vec::new();
        for item in cards {
            let id = item.get("id").and_then(Json::as_i64);
            let strength = item.get("strength").and_then(Json::as_str);
            let kind = item.get("kind").and_then(Json::as_str);
            let manual = item.get("manual").and_then(Json::as_bool);
            let scope = item.get("scope").and_then(Json::as_str);
            match (id, strength, kind, manual, scope) {
                (Some(id), Some(strength), Some(kind), Some(manual), Some(scope))
                    if id > 0
                        && (strength == "strong" || strength == "weak")
                        && allowed_kinds.contains(&kind) =>
                {
                    refs.push(RadarCardRef {
                        id,
                        strong: strength == "strong",
                        kind: kind.to_string(),
                        manual,
                        scope: scope.to_string(),
                    });
                }
                _ => return Err(Error::cache("雷达卡片引用非法")),
            }
        }
        anchors.insert(
            anchor.clone(),
            RadarBucket {
                display: display.to_string(),
                cards: refs,
            },
        );
    }
    for outputs in out_json {
        let Some(words) = outputs.as_array() else {
            return Err(Error::cache("雷达自动机输出非法"));
        };
        for word in words {
            match word.as_str() {
                Some(word) if anchors.contains_key(word) => {}
                _ => return Err(Error::cache("雷达自动机输出非法")),
            }
        }
    }
    let out: Vec<Vec<String>> = out_json
        .iter()
        .map(|words| {
            words
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    Ok((anchors, AhoCorasick { goto, out, fail }))
}

impl RadarHit {
    pub fn strength(&self) -> &'static str {
        if self.strong {
            "strong"
        } else {
            "weak"
        }
    }
}

#[derive(Clone, Debug)]
pub struct RadarHit {
    pub anchor: String,
    pub entity: String,
    pub id: i64,
    pub strong: bool,
    pub kind: String,
    pub manual: bool,
    pub scope: String,
}

pub fn scope_visible(card_scope: &str, project: &str) -> bool {
    let normalized_scope = normalize_text(card_scope);
    if normalized_scope.is_empty() || normalized_scope == "global" {
        return true;
    }
    if project.is_empty() || project == "global" {
        return false;
    }
    let normalized_project = normalize_text(project);
    normalized_scope == normalized_project
        || normalized_scope == format!("project:{normalized_project}")
}

/// _radar_hits_from_runtime: scan, verify, group, scope-filter and rank.
pub fn radar_hits_from_runtime(
    anchors: &RadarAnchors,
    ac: &AhoCorasick,
    text: &str,
    project: &str,
    candidate_limit: Option<usize>,
    deadline: Option<Instant>,
) -> Result<Vec<RadarHit>> {
    if anchors.is_empty() {
        return Ok(Vec::new());
    }
    let normalized = normalize_text(text);
    let project_norm = normalize_text(project);
    let scanned = ac.scan(&normalized, deadline)?;
    let mut raw_hits: Vec<String> = Vec::new();
    for (index, anchor) in scanned.into_iter().enumerate() {
        if deadline.is_some_and(|d| index % 64 == 0 && Instant::now() >= d) {
            return Err(Error::HookDeadlineExceeded);
        }
        if anchor_present(&anchor, &normalized) {
            raw_hits.push(anchor);
        }
    }
    if raw_hits.is_empty() {
        return Ok(Vec::new());
    }
    raw_hits.sort();
    // by_card preserves first-seen insertion order, as Python dicts do.
    let mut by_card: Vec<(i64, Vec<(String, RadarCardRef)>)> = Vec::new();
    for (index, anchor) in raw_hits.iter().enumerate() {
        if deadline.is_some_and(|d| index % 64 == 0 && Instant::now() >= d) {
            return Err(Error::HookDeadlineExceeded);
        }
        let Some(bucket) = anchors.get(anchor) else {
            return Err(Error::cache("雷达锚点引用不存在"));
        };
        for item in &bucket.cards {
            if item.id <= 0 {
                return Err(Error::cache("雷达卡片引用非法"));
            }
            match by_card.iter_mut().find(|(cid, _)| *cid == item.id) {
                Some((_, hits)) => hits.push((anchor.clone(), item.clone())),
                None => {
                    by_card.push((item.id, vec![(anchor.clone(), item.clone())]));
                    if candidate_limit.is_some_and(|limit| by_card.len() > limit) {
                        return Err(Error::HookCandidateOverflow);
                    }
                }
            }
        }
    }
    let needs_project_anchor = by_card
        .iter()
        .any(|(_, hits)| hits.iter().any(|(_, item)| !item.strong));
    let mut project_strong_cards: HashSet<i64> = HashSet::new();
    if !project_norm.is_empty() && needs_project_anchor {
        for (index, (anchor, bucket)) in anchors.iter().enumerate() {
            if deadline.is_some_and(|d| index % 64 == 0 && Instant::now() >= d) {
                return Err(Error::HookDeadlineExceeded);
            }
            if !anchor_present(anchor, &project_norm) {
                continue;
            }
            for item in &bucket.cards {
                if item.strong {
                    project_strong_cards.insert(item.id);
                }
            }
        }
    }
    let mut accepted: Vec<RadarHit> = Vec::new();
    for (cid, card_hits) in &by_card {
        for (anchor, item) in card_hits {
            if !scope_visible(&item.scope, project) {
                continue;
            }
            if item.strong {
                accepted.push(RadarHit {
                    anchor: anchor.clone(),
                    entity: anchors[anchor].display.clone(),
                    id: *cid,
                    strong: true,
                    kind: item.kind.clone(),
                    manual: item.manual,
                    scope: item.scope.clone(),
                });
                continue;
            }
            let scope = {
                let raw = item.scope.strip_prefix("project:").unwrap_or(&item.scope);
                normalize_text(raw)
            };
            let scope_match = !scope.is_empty()
                && !project_norm.is_empty()
                && (project_norm.contains(&scope) || scope.contains(&project_norm));
            let distinct: HashSet<&str> = card_hits.iter().map(|(a, _)| a.as_str()).collect();
            let second_anchor = distinct.len() >= 2;
            let project_anchor = project_strong_cards.contains(cid);
            if !(scope_match || second_anchor || project_anchor) {
                continue;
            }
            accepted.push(RadarHit {
                anchor: anchor.clone(),
                entity: anchors[anchor].display.clone(),
                id: *cid,
                strong: false,
                kind: item.kind.clone(),
                manual: item.manual,
                scope: item.scope.clone(),
            });
        }
    }
    accepted.sort_by(|a, b| {
        let ka = (
            !a.strong,
            !a.manual,
            -(py_len(&a.anchor) as i64),
            a.id,
            &a.anchor,
        );
        let kb = (
            !b.strong,
            !b.manual,
            -(py_len(&b.anchor) as i64),
            b.id,
            &b.anchor,
        );
        ka.cmp(&kb)
    });
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    for hit in accepted {
        if seen.insert(hit.id) {
            hits.push(hit);
        }
    }
    Ok(hits)
}
