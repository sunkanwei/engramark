//! Write lifecycle: save/propose/publish/reject/feedback/update/archive/
//! delete, duplicate merging, daily feedback cooldown and audits. Every state
//! transition goes through commit_source_changes with the mutation lock.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::anchors::derive_anchors;
use crate::cache::{self, all_cards, ensure_index, load_card_file};
use crate::clock::clock;
use crate::config;
use crate::durable_fs::atomic_write;
use crate::json::Json;
use crate::lock::FileLock;
use crate::mem::{
    build_structured_card, canonical_entities, card_is_current, normalize_structured_content,
    parse_card, serialize_card, Card,
};
use crate::normalize::normalize_text;
use crate::paths::Layout;
use crate::radar::scope_visible;
use crate::textops::MetaRow;
use crate::txn::commit_source_changes;
use crate::{
    trust_number, Error, Result, GET_ITEM_CAP, GET_MAX_IDS, MAX_CARD_BYTES, MAX_PUBLIC_ID,
};

pub fn log(layout: &Layout, msg: &str) {
    let logs = layout.logs();
    if crate::durable_fs::create_dir_all_private(&logs).is_err() {
        return;
    }
    let _ = crate::durable_fs::chmod_private(&logs, true);
    let line = format!("{} {msg}\n", clock().isoformat_seconds());
    let path = logs.join("core.log");
    if let Ok(mut file) = crate::durable_fs::open_private_append(&path) {
        use std::io::Write;
        let _ = file.write_all(line.as_bytes());
    }
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 1_000_000 {
            let _ = crate::durable_fs::atomic_write(&path, "");
        }
    }
}

fn max_source_id(layout: &Layout, include_legacy: bool) -> i64 {
    let mut max_id = 0i64;
    let mut scan = |dir: PathBuf| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|entry| entry.ok()) {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "mem") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()) {
                            if let Ok(id) = stem.parse::<i64>() {
                                max_id = max_id.max(id);
                            }
                        }
                    }
                }
            }
        }
    };
    scan(layout.cards());
    if include_legacy {
        scan(layout.candidates());
    }
    max_id
}

pub fn initialize_id_sequence(layout: &Layout) -> Result<i64> {
    let mut current = 0i64;
    let path = layout.id_sequence();
    if path.exists() {
        current = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok())
            .ok_or_else(|| Error::core("state/id-sequence 损坏，禁止自动复用编号"))?;
    }
    current = current.max(max_source_id(layout, true));
    if current > MAX_PUBLIC_ID {
        return Err(Error::core(format!(
            "state/id-sequence 超过安全上限 {MAX_PUBLIC_ID}"
        )));
    }
    atomic_write(&path, &format!("{current}\n")).map_err(|err| Error::core(err.to_string()))?;
    Ok(current)
}

pub fn read_id_sequence(layout: &Layout) -> Result<i64> {
    let path = layout.id_sequence();
    if !path.exists() {
        return Err(Error::core(
            "state/id-sequence 不存在，必须先执行初始化或迁移",
        ));
    }
    let value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .ok_or_else(|| Error::core("state/id-sequence 损坏，禁止自动复用编号"))?;
    if value < 0 {
        return Err(Error::core("state/id-sequence 不能为负数"));
    }
    if value > MAX_PUBLIC_ID {
        return Err(Error::core(format!(
            "state/id-sequence 超过安全上限 {MAX_PUBLIC_ID}"
        )));
    }
    Ok(value)
}

fn next_id(layout: &Layout) -> Result<i64> {
    let value = initialize_id_sequence(layout)? + 1;
    if value > MAX_PUBLIC_ID {
        return Err(Error::core(format!(
            "卡片编号已用尽（上限 {MAX_PUBLIC_ID}）"
        )));
    }
    atomic_write(&layout.id_sequence(), &format!("{value}\n"))
        .map_err(|err| Error::core(err.to_string()))?;
    Ok(value)
}

fn semantic_text(card: &Card) -> String {
    let mut text = card.title.clone();
    for line in &card.body {
        text.push('\n');
        text.push_str(line);
    }
    normalize_text(&text)
}

fn scope_key(scope: &str) -> String {
    let normalized = normalize_text(scope);
    if normalized.is_empty() || normalized == "global" {
        "global".into()
    } else {
        normalized
    }
}

fn find_duplicate(layout: &Layout, card: &Card) -> Option<Card> {
    all_cards(layout).into_iter().find(|existing| {
        (existing.status == "candidate" || existing.status == "published")
            && scope_key(&existing.scope) == scope_key(&card.scope)
            && semantic_text(existing) == semantic_text(card)
    })
}

fn persist_new_card(
    layout: &Layout,
    card: &mut Card,
    status: &str,
    source: &str,
    lock: bool,
) -> Result<Card> {
    card.status = status.to_string();
    card.updated = clock().today();
    if !source.is_empty() {
        card.source = source.to_string();
    }
    if lock {
        card.lock = true;
    }
    if card.lock && card.trust < 6 {
        card.trust = 6;
    }
    ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    if let Some(mut duplicate) = find_duplicate(layout, card) {
        let mut changed = false;
        if status == "published" {
            if duplicate.status == "candidate" {
                duplicate.status = "published".into();
                duplicate.source = if source.is_empty() {
                    "user".into()
                } else {
                    source.into()
                };
                duplicate.card_type = card.card_type.clone();
                duplicate.importance = card.importance;
                duplicate.trust = card.trust;
                duplicate.lock = card.lock;
                duplicate.updated = clock().today();
                changed = true;
            }
            let merged =
                canonical_entities(&[duplicate.entities.clone(), card.entities.clone()].concat());
            if merged != duplicate.entities {
                duplicate.entities = merged;
                changed = true;
            }
            if lock && !duplicate.lock {
                duplicate.lock = true;
                duplicate.trust = 6;
                changed = true;
            }
            if changed {
                commit_source_changes(
                    layout,
                    "deduplicate-merge",
                    BTreeMap::from([(
                        layout.card_path(duplicate.id),
                        serialize_card(&duplicate).into_bytes(),
                    )]),
                )?;
            }
        }
        duplicate.deduplicated = true;
        duplicate.unchanged = !changed;
        return Ok(duplicate);
    }
    card.id = next_id(layout)?;
    card.deduplicated = false;
    commit_source_changes(
        layout,
        "create-card",
        BTreeMap::from([(layout.card_path(card.id), serialize_card(card).into_bytes())]),
    )?;
    Ok(card.clone())
}

pub fn write_new_card(
    layout: &Layout,
    text: &str,
    status: &str,
    source: &str,
    lock: bool,
) -> Result<Card> {
    let mut card = parse_card(text)?;
    persist_new_card(layout, &mut card, status, source, lock)
}

#[allow(clippy::too_many_arguments)]
pub fn write_structured_card(
    layout: &Layout,
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
    let mut card = build_structured_card(
        title, body, entities, card_type, scope, project, status, source, lock,
    )?;
    persist_new_card(layout, &mut card, status, source, lock)
}

fn require_visible_scope(card: &Card, project: Option<&str>) -> Result<()> {
    if let Some(project) = project {
        if !scope_visible(&card.scope, project) {
            return Err(Error::core(format!("当前范围内不存在记忆 {}", card.id)));
        }
    }
    Ok(())
}

pub fn publish(layout: &Layout, cid: i64, project: Option<&str>) -> Result<Card> {
    ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    let path = layout.card_path(cid);
    if !path.exists() {
        return Err(Error::core(format!("候选记忆 {cid} 不存在")));
    }
    let mut card = load_card_file(&path)?;
    require_visible_scope(&card, project)?;
    if card.status == "published" {
        card.unchanged = true;
        return Ok(card);
    }
    if card.status != "candidate" {
        return Err(Error::core(format!("记忆 {cid} 不是待发布的候选记忆")));
    }
    card.status = "published".into();
    card.updated = clock().today();
    commit_source_changes(
        layout,
        "publish-card",
        BTreeMap::from([(path, serialize_card(&card).into_bytes())]),
    )?;
    Ok(card)
}

fn audit_log(layout: &Layout, record: Json) {
    let _audit = match FileLock::acquire(layout, "audit", false, None) {
        Ok(lock) => lock,
        Err(_) => return,
    };
    let path = layout.logs().join("audit.log");
    if let Ok(mut file) = crate::durable_fs::open_private_append(&path) {
        use std::io::Write;
        let _ = file.write_all(format!("{}\n", record.dumps()).as_bytes());
    }
}

pub fn reject(layout: &Layout, cid: i64, project: Option<&str>) -> Result<Card> {
    ensure_index(layout)?;
    let audit_source;
    let card;
    {
        let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
        let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
        let path = layout.card_path(cid);
        if !path.exists() {
            return Err(Error::core(format!("候选记忆 {cid} 不存在")));
        }
        let mut loaded = load_card_file(&path)?;
        require_visible_scope(&loaded, project)?;
        if loaded.status == "tombstone" && loaded.source == "system:rejected" {
            loaded.unchanged = true;
            return Ok(loaded);
        }
        if loaded.status != "candidate" {
            return Err(Error::core(format!("记忆 {cid} 不是可丢弃的候选记忆")));
        }
        let prefix = loaded.source.split(':').next().unwrap_or("");
        audit_source = if ["user", "self", "system", "external"].contains(&prefix) {
            prefix.to_string()
        } else {
            "unknown".to_string()
        };
        loaded.status = "tombstone".into();
        loaded.card_type = "fact".into();
        loaded.importance = 0;
        loaded.trust = 0;
        loaded.updated = clock().today();
        loaded.entities = Vec::new();
        loaded.source = "system:rejected".into();
        loaded.lock = false;
        loaded.last_used = String::new();
        loaded.valid_from = String::new();
        loaded.valid_to = String::new();
        loaded.supersedes = Vec::new();
        loaded.title = format!("已拒绝的候选记忆 {cid}。");
        loaded.body = Vec::new();
        commit_source_changes(
            layout,
            "reject-card",
            BTreeMap::from([(path, serialize_card(&loaded).into_bytes())]),
        )?;
        card = loaded;
    }
    audit_log(
        layout,
        crate::jobject! {
            "time" => clock().isoformat_seconds(),
            "action" => "reject",
            "id" => cid,
            "source" => audit_source,
        },
    );
    Ok(card)
}

pub fn feedback(layout: &Layout, cid: i64, signal: &str, project: Option<&str>) -> Result<Json> {
    if signal != "+" && signal != "-" {
        return Err(Error::core("反馈信号只能是 + 或 -"));
    }
    let metas: BTreeMap<i64, MetaRow> = cache::get_meta(layout, &[cid])?
        .into_iter()
        .map(|meta| (meta.id, meta))
        .collect();
    let Some(m) = metas.get(&cid) else {
        return Err(Error::core(format!("卡片 @{cid} 不存在")));
    };
    if let Some(project) = project {
        if !scope_visible(&m.scope, project) {
            return Err(Error::core(format!("当前范围内不存在记忆 {cid}")));
        }
    }
    if m.status != "published" {
        return Err(Error::core(format!(
            "@{cid} 不是已发布记忆，不能接受检索反馈"
        )));
    }
    if m.lock && signal == "-" {
        return Err(Error::core(format!(
            "@{cid} 已被用户锁定，agent 反馈不得削弱；如确有问题请直接告诉用户"
        )));
    }
    if m.lock {
        return Ok(crate::jobject! {
            "id" => cid,
            "trust" => trust_number(m.t),
        });
    }
    let path = layout.card_path(cid);
    let fb_mark = layout.feedback_state().join(format!("{cid}.mark"));
    let card;
    {
        ensure_index(layout)?;
        let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
        let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
        let mut loaded = load_card_file(&path)?;
        require_visible_scope(&loaded, project)?;
        if loaded.status != "published" {
            return Err(Error::core(format!(
                "@{cid} 已不再是发布状态，不能接受检索反馈"
            )));
        }
        if loaded.lock && signal == "-" {
            return Err(Error::core(format!(
                "@{cid} 已被用户锁定，agent 反馈不得削弱；如确有问题请直接告诉用户"
            )));
        }
        crate::durable_fs::create_dir_all_private(&layout.feedback_state())
            .map_err(|err| Error::core(err.to_string()))?;
        crate::durable_fs::chmod_private(&layout.feedback_state(), true)
            .map_err(|err| Error::core(err.to_string()))?;
        if fb_mark.exists()
            && std::fs::read_to_string(&fb_mark)
                .map(|text| text.trim() == clock().today())
                .unwrap_or(false)
        {
            return Err(Error::core(format!(
                "@{cid} 今天已反馈过（冷却中），避免刷分"
            )));
        }
        let delta = if signal == "+" { 1 } else { -2 };
        loaded.trust = (loaded.trust + delta).clamp(0, 6);
        loaded.updated = clock().today();
        commit_source_changes(
            layout,
            "feedback",
            BTreeMap::from([
                (path, serialize_card(&loaded).into_bytes()),
                (fb_mark, format!("{}\n", clock().today()).into_bytes()),
            ]),
        )?;
        card = loaded;
    }
    audit_log(
        layout,
        crate::jobject! {
            "time" => clock().isoformat_seconds(),
            "action" => "feedback",
            "id" => cid,
            "signal" => signal,
            "new_t" => crate::trust_text(card.trust),
        },
    );
    Ok(crate::jobject! {
        "id" => cid,
        "trust" => trust_number(card.trust),
    })
}

#[derive(Default)]
pub struct UpdateFields {
    pub title: Option<String>,
    pub body: Option<Vec<String>>,
    pub entities: Option<Vec<String>>,
    pub card_type: Option<String>,
}

fn update_card_values(
    layout: &Layout,
    cid: i64,
    fields: UpdateFields,
    project: Option<&str>,
) -> Result<Card> {
    ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    let path = layout.card_path(cid);
    if !path.exists() {
        return Err(Error::core(format!("记忆 {cid} 不存在")));
    }
    let mut current = load_card_file(&path)?;
    require_visible_scope(&current, project)?;
    if current.status != "published" {
        return Err(Error::core(format!("记忆 {cid} 不是可更新的正式记忆")));
    }
    let next_title = fields
        .title
        .clone()
        .unwrap_or_else(|| current.title.clone());
    let next_body = fields.body.clone().unwrap_or_else(|| current.body.clone());
    let next_entities = fields
        .entities
        .clone()
        .unwrap_or_else(|| current.entities.clone());
    let next_type = fields
        .card_type
        .clone()
        .unwrap_or_else(|| current.card_type.clone());
    if current.title == next_title
        && current.body == next_body
        && current.entities == next_entities
        && current.card_type == next_type
    {
        current.unchanged = true;
        return Ok(current);
    }
    current.title = next_title;
    current.body = next_body;
    current.entities = next_entities;
    current.card_type = next_type;
    current.updated = clock().today();
    let payload = serialize_card(&current).into_bytes();
    if payload.len() > MAX_CARD_BYTES {
        return Err(Error::core(format!(
            "记忆内容超过 {MAX_CARD_BYTES} 字节上限"
        )));
    }
    commit_source_changes(layout, "update-card", BTreeMap::from([(path, payload)]))?;
    Ok(current)
}

pub fn update_card(layout: &Layout, cid: i64, text: &str) -> Result<Card> {
    let replacement = parse_card(text)?;
    if replacement.id != 0 && replacement.id != cid {
        return Err(Error::core("更新文本中的编号必须为 0 或目标记忆编号"));
    }
    update_card_values(
        layout,
        cid,
        UpdateFields {
            title: Some(replacement.title),
            body: Some(replacement.body),
            entities: Some(replacement.entities),
            card_type: Some(replacement.card_type),
        },
        None,
    )
}

/// Field-wise update used by MCP memory_update; each present field is first
/// validated by normalize_structured_content with placeholder fillers.
pub fn update_card_fields(
    layout: &Layout,
    cid: i64,
    title: Option<&str>,
    body: Option<&str>,
    entities: Option<&[String]>,
    card_type: Option<&str>,
    project: Option<&str>,
) -> Result<Card> {
    let mut fields = UpdateFields::default();
    if let Some(title) = title {
        fields.title = Some(normalize_structured_content(title, "", &[], "fact")?.0);
    }
    if let Some(body) = body {
        fields.body = Some(normalize_structured_content("临时标题", body, &[], "fact")?.1);
    }
    if let Some(entities) = entities {
        fields.entities = Some(normalize_structured_content("临时标题", "", entities, "fact")?.2);
    }
    if let Some(card_type) = card_type {
        fields.card_type = Some(normalize_structured_content("临时标题", "", &[], card_type)?.3);
    }
    update_card_values(layout, cid, fields, project)
}

pub fn archive_card(layout: &Layout, cid: i64, project: Option<&str>) -> Result<Card> {
    ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    let path = layout.card_path(cid);
    if !path.exists() {
        return Err(Error::core(format!("记忆 {cid} 不存在")));
    }
    let mut card = load_card_file(&path)?;
    require_visible_scope(&card, project)?;
    if card.status == "archived" {
        card.unchanged = true;
        return Ok(card);
    }
    if card.status != "published" {
        return Err(Error::core(format!("记忆 {cid} 不是可归档的正式记忆")));
    }
    card.status = "archived".into();
    card.updated = clock().today();
    commit_source_changes(
        layout,
        "archive-card",
        BTreeMap::from([(path, serialize_card(&card).into_bytes())]),
    )?;
    Ok(card)
}

pub fn tombstone_card(
    layout: &Layout,
    cid: i64,
    confirmed: bool,
    project: Option<&str>,
) -> Result<Card> {
    ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    let path = layout.card_path(cid);
    if !path.exists() {
        return Err(Error::core(format!("记忆 {cid} 不存在")));
    }
    let mut card = load_card_file(&path)?;
    require_visible_scope(&card, project)?;
    if card.status == "tombstone" {
        card.unchanged = true;
        return Ok(card);
    }
    if card.status == "published" && !confirmed {
        return Err(Error::core("删除已发布卡片必须显式确认"));
    }
    card.status = "tombstone".into();
    card.card_type = "fact".into();
    card.importance = 0;
    card.trust = 0;
    card.updated = clock().today();
    card.entities = Vec::new();
    card.source = "system:deleted".into();
    card.lock = false;
    card.last_used = String::new();
    card.valid_from = String::new();
    card.valid_to = String::new();
    card.supersedes = Vec::new();
    card.title = format!("已删除的记忆 {cid}。");
    card.body = Vec::new();
    commit_source_changes(
        layout,
        "tombstone-card",
        BTreeMap::from([(path, serialize_card(&card).into_bytes())]),
    )?;
    Ok(card)
}

pub struct GetCardOut {
    pub id: i64,
    pub meta: MetaRow,
    pub text: String,
    pub truncated: bool,
}

/// get_cards: bounded read (1..=5) with the daily last-used write-back.
pub fn get_cards(layout: &Layout, ids: &[i64], project: Option<&str>) -> Result<Vec<GetCardOut>> {
    if ids.len() > GET_MAX_IDS {
        return Err(Error::core(format!(
            "单次最多取 {GET_MAX_IDS} 张（渐进披露纪律），收到 {} 个 id",
            ids.len()
        )));
    }
    let mut out = Vec::new();
    let mut touch: Vec<i64> = Vec::new();
    if !ids.is_empty() {
        // The shared swap lock covers both cache and source reads so one call
        // cannot combine two generations.
        let reader = cache::cache_reader(layout)?;
        let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut stmt = reader
            .conn
            .prepare(&format!("SELECT * FROM meta WHERE id IN ({marks})"))
            .map_err(|err| Error::cache(err.to_string()))?;
        let sql_params: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(sql_params.as_slice(), cache::row_to_meta)
            .map_err(|err| Error::cache(err.to_string()))?;
        let mut metas: BTreeMap<i64, MetaRow> = BTreeMap::new();
        for row in rows {
            let meta = row.map_err(|err| Error::cache(err.to_string()))?;
            metas.insert(meta.id, meta);
        }
        for cid in ids {
            let Some(m) = metas.get(cid) else {
                continue;
            };
            if let Some(project) = project {
                if !scope_visible(&m.scope, project) {
                    continue;
                }
            }
            let path = layout.card_path(*cid);
            if !path.exists() {
                continue;
            }
            let card = load_card_file(&path)?;
            let mut text = card.full_text();
            let mut truncated = false;
            if text.len() > GET_ITEM_CAP {
                let mut end = GET_ITEM_CAP;
                while end > 0 && !text.is_char_boundary(end) {
                    end -= 1;
                }
                text = text[..end].to_string();
                truncated = true;
            }
            if m.status == "published" && card.last_used != clock().today() {
                touch.push(*cid);
            }
            out.push(GetCardOut {
                id: *cid,
                meta: m.clone(),
                text,
                truncated,
            });
        }
    }
    if !touch.is_empty() {
        ensure_index(layout)?;
        let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
        let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
        let mut writes: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
        for cid in &touch {
            let path = layout.card_path(*cid);
            if !path.exists() {
                continue;
            }
            let mut card = load_card_file(&path)?;
            if card.status == "published" && card.last_used != clock().today() {
                card.last_used = clock().today();
                writes.insert(path, serialize_card(&card).into_bytes());
            }
        }
        if !writes.is_empty() {
            commit_source_changes(layout, "touch-last-used", writes)?;
        }
    }
    Ok(out)
}

fn audit_locked(layout: &Layout, project: Option<&str>) -> Result<Json> {
    let mut stale = Vec::new();
    let mut unused = Vec::new();
    let mut all_known = all_cards(layout);
    if let Some(project) = project {
        all_known.retain(|card| scope_visible(&card.scope, project));
    }
    for card in &all_known {
        if card.status != "published" || card.lock {
            continue;
        }
        let f = card.freshness();
        let reference = if card.last_used.is_empty() {
            card.updated.clone()
        } else {
            card.last_used.clone()
        };
        let days = crate::mem::parse_iso_date(&reference)
            .and_then(|ref_ordinal| {
                crate::mem::parse_iso_date(&clock().today()).map(|today| today - ref_ordinal)
            })
            .unwrap_or(0);
        if card.importance <= 1 && f < 0.3 {
            stale.push(crate::jobject! {
                "id" => card.id,
                "title" => card.title.clone(),
                "I" => card.importance,
                "F" => f,
            });
        } else if days > 90 {
            unused.push(crate::jobject! {
                "id" => card.id,
                "title" => card.title.clone(),
                "days" => days,
            });
        }
    }
    let candidates: Vec<&Card> = all_known
        .iter()
        .filter(|card| card.status == "candidate")
        .collect();
    let cfg = config::load_config(&layout.home);
    let superseded: std::collections::HashSet<i64> = all_known
        .iter()
        .filter(|card| card.status == "published" && card_is_current(card))
        .flat_map(|card| card.supersedes.iter().copied())
        .collect();
    // Insertion order mirrors the Python dict (first appearance per card).
    let mut strong_by_anchor: Vec<(String, Vec<i64>)> = Vec::new();
    for card in &all_known {
        if card.status != "published" || !card_is_current(card) || superseded.contains(&card.id) {
            continue;
        }
        for anchor in derive_anchors(card, &cfg) {
            if anchor.strong && anchor.manual {
                match strong_by_anchor
                    .iter_mut()
                    .find(|(norm, _)| *norm == anchor.norm)
                {
                    Some((_, ids)) => ids.push(card.id),
                    None => strong_by_anchor.push((anchor.norm.clone(), vec![card.id])),
                }
            }
        }
    }
    let mut conflicts = Vec::new();
    for (anchor, ids) in &strong_by_anchor {
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() > 1 {
            conflicts.push(crate::jobject! {
                "anchor" => anchor.clone(),
                "ids" => Json::Array(unique.into_iter().map(Json::Int).collect()),
            });
        }
    }
    Ok(crate::jobject! {
        "stale" => Json::Array(stale),
        "unused_90d" => Json::Array(unused),
        "candidates" => Json::Array(candidates.iter().map(|card| crate::jobject! {
            "id" => card.id,
            "title" => card.title.clone(),
            "source" => card.source.clone(),
        }).collect()),
        "possible_conflicts" => Json::Array(conflicts),
    })
}

pub fn audit(layout: &Layout, project: Option<&str>) -> Result<Json> {
    ensure_index(layout)?;
    let _shared = FileLock::acquire(layout, "cache.swap", true, None)?;
    audit_locked(layout, project)
}

pub fn candidate_list(layout: &Layout, project: Option<&str>) -> Result<Vec<Card>> {
    ensure_index(layout)?;
    let _shared = FileLock::acquire(layout, "cache.swap", true, None)?;
    let mut cards: Vec<Card> = all_cards(layout)
        .into_iter()
        .filter(|card| card.status == "candidate")
        .collect();
    if let Some(project) = project {
        if !project.is_empty() {
            cards.retain(|card| scope_visible(&card.scope, project));
        }
    }
    Ok(cards)
}
