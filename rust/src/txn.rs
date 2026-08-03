//! Journal v1 (MEMTXN): write-ahead evidence for multi-file card writes,
//! recovery of Python-pending transactions, and the commit pipeline. Journals
//! written here are validatable by the frozen Python reference and vice versa.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;

use crate::cache::{self, load_cards_for_cache, probe_runtime};
use crate::clock::{clock, crash_point};
use crate::durable_fs::{atomic_write_bytes, durable_unlink};
use crate::hash::{journal_checksum, sha256_hex};
use crate::json::Json;
use crate::lock::FileLock;
use crate::mem::parse_card;
use crate::paths::Layout;
use crate::{Error, Result};

pub fn has_pending_transactions(layout: &Layout) -> bool {
    std::fs::read_dir(layout.transactions())
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "txn"))
        })
        .unwrap_or(false)
}

fn sorted_journals(layout: &Layout) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(layout.transactions())
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "txn"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

/// _write_journal: snapshot before/after bytes for every write target.
fn write_journal(
    layout: &Layout,
    operation: &str,
    writes: &BTreeMap<PathBuf, Vec<u8>>,
    generation: i64,
    op_uuid: &str,
) -> Result<PathBuf> {
    let mut entries = Vec::new();
    for (path, after) in writes {
        let before = std::fs::read(path).unwrap_or_default();
        let before_exists = path.exists();
        entries.push(crate::jobject! {
            "path" => crate::cache::relative_path(layout, path),
            "before_exists" => before_exists,
            "after_exists" => true,
            "before_hash" => if before_exists { sha256_hex(&before) } else { String::new() },
            "after_hash" => sha256_hex(after),
            "before_b64" => base64::engine::general_purpose::STANDARD.encode(&before),
            "after_b64" => base64::engine::general_purpose::STANDARD.encode(after),
        });
    }
    let cards_root = layout.cards();
    let mut card_ids: Vec<i64> = writes
        .keys()
        .filter(|path| path.parent() == Some(cards_root.as_path()))
        .filter_map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|stem| stem.parse::<i64>().ok())
        })
        .collect();
    card_ids.sort_unstable();
    let mut payload = crate::jobject! {
        "version" => 1i64,
        "uuid" => op_uuid,
        "operation" => operation,
        "created_at_ns" => clock().unix_nanos(),
        "card_ids" => Json::Array(card_ids.into_iter().map(Json::Int).collect()),
        "target_generation" => generation,
        "files" => Json::Array(entries),
    };
    let checksum = journal_checksum(&payload);
    if let Json::Object(ref mut pairs) = payload {
        pairs.push(("checksum".into(), Json::Str(checksum)));
    }
    let path = layout.transactions().join(format!("{op_uuid}.txn"));
    crate::durable_fs::atomic_write(&path, &format!("{}\n", payload.dumps_canonical()))
        .map_err(|err| Error::core(format!("事务日志写入失败：{err}")))?;
    Ok(path)
}

fn feedback_mark_id(layout: &Layout, path: &Path) -> Option<i64> {
    if path.parent() != Some(layout.feedback_state().as_path()) {
        return None;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".mark"))
        .filter(|stem| !stem.is_empty() && stem.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|stem| stem.parse::<i64>().ok())
}

fn validate_transaction_writes(
    layout: &Layout,
    operation: &str,
    writes: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    let (cards, _) = load_cards_for_cache(layout)?;
    let mut by_id: BTreeMap<i64, crate::mem::Card> =
        cards.into_iter().map(|card| (card.id, card)).collect();
    let cards_root = crate::paths::resolve_lenient(&layout.cards());
    for (path, data) in writes {
        let resolved = crate::paths::resolve_lenient(path);
        if operation == "feedback" && feedback_mark_id(layout, path).is_some() {
            let text = std::str::from_utf8(data)
                .map_err(|_| Error::core(format!("事务目标 {} 不是 UTF-8", file_name(path))))?;
            if !text.ends_with('\n')
                || text.lines().count() != 1
                || crate::mem::parse_iso_date(text.trim_end_matches('\n')).is_none()
            {
                return Err(Error::core(format!(
                    "反馈冷却标记 {} 内容非法",
                    file_name(path)
                )));
            }
            continue;
        }
        if !resolved.starts_with(&cards_root) || path.parent() != Some(layout.cards().as_path()) {
            return Err(Error::core(format!(
                "事务目标不在 cards/：{}",
                path.display()
            )));
        }
        let text = String::from_utf8(data.clone())
            .map_err(|_| Error::core(format!("事务目标 {} 不是 UTF-8", file_name(path))))?;
        let card = parse_card(&text)?;
        let stem_ok = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<i64>().ok())
            .is_some_and(|file_id| file_id == card.id);
        if !stem_ok {
            return Err(Error::core(format!(
                "事务目标 {} 与卡头 @{} 不一致",
                file_name(path),
                card.id
            )));
        }
        by_id.insert(card.id, card);
    }
    if operation == "feedback" {
        let card_ids: Vec<i64> = writes
            .keys()
            .filter(|path| path.parent() == Some(layout.cards().as_path()))
            .filter_map(|path| path.file_stem()?.to_str()?.parse().ok())
            .collect();
        let mark_ids: Vec<i64> = writes
            .keys()
            .filter_map(|path| feedback_mark_id(layout, path))
            .collect();
        if card_ids.len() != 1 || mark_ids != card_ids {
            return Err(Error::core(
                "feedback 事务必须同时且仅写入同编号卡片与冷却标记",
            ));
        }
    }
    let known: std::collections::HashSet<i64> = by_id.keys().copied().collect();
    for card in by_id.values() {
        let missing: Vec<i64> = card
            .supersedes
            .iter()
            .filter(|cid| !known.contains(cid))
            .copied()
            .collect();
        if !missing.is_empty() {
            let mut missing = missing;
            missing.sort_unstable();
            missing.dedup();
            return Err(Error::core(format!(
                "@{} 引用了不存在的卡片 {missing:?}",
                card.id
            )));
        }
    }
    // Cycle check over the merged graph (insertion order like Python).
    let mut marks: std::collections::HashMap<i64, u8> = std::collections::HashMap::new();
    fn visit(
        cid: i64,
        by_id: &BTreeMap<i64, crate::mem::Card>,
        marks: &mut std::collections::HashMap<i64, u8>,
    ) -> Result<()> {
        match marks.get(&cid) {
            Some(1) => return Err(Error::core(format!("supersedes 形成环，涉及 @{cid}"))),
            Some(2) => return Ok(()),
            _ => {}
        }
        marks.insert(cid, 1);
        if let Some(card) = by_id.get(&cid) {
            for next in &card.supersedes {
                visit(*next, by_id, marks)?;
            }
        }
        marks.insert(cid, 2);
        Ok(())
    }
    for cid in by_id.keys() {
        visit(*cid, &by_id, &mut marks)?;
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// _commit_source_changes: journal → sources → cache sync → journal removal.
/// If the cache commit fails after sources persisted, the error is reported
/// but the journal remains — recovery will finish the transaction later; the
/// caller must present this as an undecided result, never as rolled back.
pub fn commit_source_changes(
    layout: &Layout,
    operation: &str,
    writes: BTreeMap<PathBuf, Vec<u8>>,
) -> Result<String> {
    if writes.is_empty() {
        return Ok(String::new());
    }
    validate_transaction_writes(layout, operation, &writes)?;
    let runtime = probe_runtime()?;
    let generation = cache::read_generation_unlocked(layout) + 1;
    let op_uuid = clock().uuid4();
    let journal = write_journal(layout, operation, &writes, generation, &op_uuid)?;
    crash_point("after-journal");
    for (path, data) in &writes {
        if std::env::var("ENGRAMARK_CRASH_STAGE").as_deref() == Ok("disk-full-source") {
            return Err(Error::core(
                "写入源文件失败：No space left on device (os error 28)",
            ));
        }
        atomic_write_bytes(path, data)
            .map_err(|err| Error::core(format!("写入源文件失败：{err}")))?;
        crash_point("after-source");
    }
    let (cards, invalid) = load_cards_for_cache(layout)?;
    {
        let conn = cache::open_write(
            &layout.index(),
            std::time::Duration::from_secs_f64(crate::lock::LOCK_TIMEOUT),
        )
        .map_err(|err| Error::cache(err.to_string()))?;
        cache::apply_write_pragmas(&conn).map_err(|err| Error::cache(err.to_string()))?;
        if std::env::var("ENGRAMARK_CRASH_STAGE").as_deref() == Ok("disk-full-cache") {
            return Err(Error::cache("增量更新失败：database or disk is full"));
        }
        cache::sync_cache(
            &conn, layout, &cards, &invalid, generation, &op_uuid, &runtime,
        )?;
    }
    crash_point("after-cache");
    durable_unlink(&journal).map_err(|err| Error::core(format!("删除事务日志失败：{err}")))?;
    Ok(op_uuid)
}

fn load_journal(path: &Path) -> Result<Json> {
    if crate::paths::is_link_like(path) {
        return Err(Error::core(format!(
            "事务日志 {} 不能是符号链接，需人工处理",
            file_name(path)
        )));
    }
    let text = std::fs::read_to_string(path)
        .map_err(|_| Error::core(format!("事务日志 {} 无法解析，需人工处理", file_name(path))))?;
    let payload = Json::parse(&text)
        .map_err(|_| Error::core(format!("事务日志 {} 无法解析，需人工处理", file_name(path))))?;
    let version_ok = payload.get("version").and_then(Json::as_i64) == Some(1);
    let checksum_ok = payload
        .get("checksum")
        .and_then(Json::as_str)
        .is_some_and(|checksum| checksum == journal_checksum(&payload));
    if !version_ok || !checksum_ok {
        return Err(Error::core(format!(
            "事务日志 {} 校验失败，需人工处理",
            file_name(path)
        )));
    }
    let uuid_ok = payload
        .get("uuid")
        .and_then(Json::as_str)
        .is_some_and(|uuid| path.file_stem().and_then(|s| s.to_str()) == Some(uuid));
    if !uuid_ok || payload.get("files").and_then(Json::as_array).is_none() {
        return Err(Error::core(format!(
            "事务日志 {} 身份字段非法",
            file_name(path)
        )));
    }
    Ok(payload)
}

fn journal_source_path(
    layout: &Layout,
    item: &Json,
    uuid: &str,
    operation: &str,
) -> Result<PathBuf> {
    let rel = item
        .get("path")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::core(format!("事务 {uuid} 的路径字段非法")))?;
    let relative = Path::new(rel);
    let mut components = relative.components();
    let cards = components.next();
    let file = components.next();
    let exact = matches!(cards, Some(std::path::Component::Normal(value)) if value == "cards")
        && matches!(file, Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    let path = layout.home.join(relative);
    let stem_ok = exact
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".mem"))
            .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|byte| byte.is_ascii_digit()));
    if stem_ok {
        return Ok(path);
    }
    let mut components = relative.components();
    let state = components.next();
    let feedback = components.next();
    let file = components.next();
    let mark_exact = operation == "feedback"
        && matches!(state, Some(std::path::Component::Normal(value)) if value == "state")
        && matches!(feedback, Some(std::path::Component::Normal(value)) if value == "feedback")
        && matches!(file, Some(std::path::Component::Normal(_)))
        && components.next().is_none()
        && feedback_mark_id(layout, &path).is_some();
    if mark_exact {
        return Ok(path);
    }
    Err(Error::core(format!(
        "事务 {uuid} 的路径不在允许的真源范围：{rel}"
    )))
}

/// recover_transactions: deterministic handling of the four states plus
/// mixed multi-file states; anything neither-old-nor-new stops recovery and
/// preserves evidence.
pub fn recover_transactions(layout: &Layout) -> Result<Vec<Json>> {
    layout
        .ensure()
        .map_err(|err| Error::core(err.to_string()))?;
    if !has_pending_transactions(layout) {
        return Ok(Vec::new());
    }
    let runtime = probe_runtime()?;
    let mut reports = Vec::new();
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    for journal_path in sorted_journals(layout) {
        let tx = load_journal(&journal_path)?;
        let uuid = tx
            .get("uuid")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let files: Vec<Json> = tx
            .get("files")
            .and_then(Json::as_array)
            .map(|items| items.to_vec())
            .unwrap_or_default();
        let operation = tx.get("operation").and_then(Json::as_str).unwrap_or("");
        let mut paths = Vec::with_capacity(files.len());
        let mut unique_paths = std::collections::HashSet::new();
        for item in &files {
            let path = journal_source_path(layout, item, &uuid, operation)?;
            if !unique_paths.insert(path.clone()) {
                return Err(Error::core(format!("事务 {uuid} 包含重复路径")));
            }
            paths.push(path);
        }
        let mut after_writes = BTreeMap::new();
        for (item, path) in files.iter().zip(&paths) {
            let after_b64 = item.get("after_b64").and_then(Json::as_str).unwrap_or("");
            let data = base64::engine::general_purpose::STANDARD
                .decode(after_b64)
                .map_err(|_| Error::core(format!("事务 {uuid} 的恢复载荷校验失败")))?;
            if sha256_hex(&data) != item.get("after_hash").and_then(Json::as_str).unwrap_or("") {
                return Err(Error::core(format!("事务 {uuid} 的恢复载荷校验失败")));
            }
            after_writes.insert(path.clone(), data);
        }
        let mut legacy_feedback_date = None;
        if operation == "feedback" {
            let card_ids: Vec<i64> = paths
                .iter()
                .filter(|path| path.parent() == Some(layout.cards().as_path()))
                .filter_map(|path| path.file_stem()?.to_str()?.parse().ok())
                .collect();
            let mark_ids: Vec<i64> = paths
                .iter()
                .filter_map(|path| feedback_mark_id(layout, path))
                .collect();
            let legacy = card_ids.len() == 1 && mark_ids.is_empty();
            if !legacy && (card_ids.len() != 1 || mark_ids != card_ids) {
                return Err(Error::core(format!("事务 {uuid} 的 feedback 文件集合非法")));
            }
            if legacy {
                let card_path = paths
                    .iter()
                    .find(|path| path.parent() == Some(layout.cards().as_path()))
                    .expect("validated legacy feedback card");
                let card_text = std::str::from_utf8(
                    after_writes
                        .get(card_path)
                        .expect("decoded legacy feedback payload"),
                )
                .map_err(|_| Error::core(format!("事务 {uuid} 的恢复载荷不是 UTF-8")))?;
                let card = parse_card(card_text)?;
                if crate::mem::parse_iso_date(&card.updated).is_none() {
                    return Err(Error::core(format!("事务 {uuid} 的反馈日期非法")));
                }
                legacy_feedback_date = Some((card.id, card.updated));
                validate_transaction_writes(layout, "legacy-feedback", &after_writes)?;
            } else {
                validate_transaction_writes(layout, operation, &after_writes)?;
            }
        } else {
            validate_transaction_writes(layout, operation, &after_writes)?;
        }
        let mut applied = false;
        if layout.index().exists() {
            if let Ok(conn) =
                cache::open_readonly(&layout.index(), std::time::Duration::from_secs(5))
            {
                applied = conn
                    .query_row(
                        "SELECT 1 FROM applied_ops WHERE uuid=?",
                        rusqlite::params![uuid],
                        |_| Ok(()),
                    )
                    .is_ok();
            }
        }
        let mut states = Vec::new();
        for (item, path) in files.iter().zip(&paths) {
            let rel = item.get("path").and_then(Json::as_str).unwrap_or("");
            let current = cache::source_hash(path);
            let before_hash = item.get("before_hash").and_then(Json::as_str).unwrap_or("");
            let after_hash = item.get("after_hash").and_then(Json::as_str).unwrap_or("");
            let before_exists = item
                .get("before_exists")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let after_exists = item
                .get("after_exists")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            if current == before_hash && path.exists() == before_exists {
                states.push("old");
            } else if current == after_hash && path.exists() == after_exists {
                states.push("new");
            } else {
                return Err(Error::core(format!(
                    "事务 {uuid} 的 {rel} 既非修改前也非修改后版本；停止自动恢复"
                )));
            }
        }
        if states.iter().all(|state| *state == "old") && !applied {
            durable_unlink(&journal_path).map_err(|err| Error::core(err.to_string()))?;
            reports.push(crate::jobject! {
                "uuid" => uuid,
                "action" => "aborted-before-source",
            });
            continue;
        }
        if states.iter().all(|state| *state == "new") && applied {
            if let Some((cid, date)) = &legacy_feedback_date {
                let mark = layout.feedback_state().join(format!("{cid}.mark"));
                atomic_write_bytes(&mark, format!("{date}\n").as_bytes())
                    .map_err(|err| Error::core(err.to_string()))?;
            }
            durable_unlink(&journal_path).map_err(|err| Error::core(err.to_string()))?;
            reports.push(crate::jobject! {
                "uuid" => uuid,
                "action" => "already-committed",
            });
            continue;
        }
        #[allow(clippy::needless_late_init)]
        let action: &str;
        if !(states.iter().all(|state| *state == "old") && applied) {
            for (path, state) in paths.iter().zip(&states) {
                if *state != "old" {
                    continue;
                }
                let data = after_writes
                    .get(path)
                    .expect("validated transaction payload");
                atomic_write_bytes(path, data).map_err(|err| Error::core(err.to_string()))?;
            }
            action = "completed-source-and-cache";
        } else {
            action = "repaired-cache-to-source";
        }
        let (cards, invalid) = load_cards_for_cache(layout)?;
        let target_generation = tx
            .get("target_generation")
            .and_then(Json::as_i64)
            .unwrap_or(0);
        let generation = (cache::read_generation_unlocked(layout) + 1).max(target_generation);
        let mut cache_ok = layout.index().exists();
        if cache_ok {
            match cache::open_write(
                &layout.index(),
                std::time::Duration::from_secs_f64(crate::lock::LOCK_TIMEOUT),
            ) {
                Ok(conn) => {
                    if cache::apply_write_pragmas(&conn).is_err()
                        || cache::sync_cache(
                            &conn, layout, &cards, &invalid, generation, &uuid, &runtime,
                        )
                        .is_err()
                    {
                        drop(conn);
                        let _ = durable_unlink(&layout.index());
                        cache_ok = false;
                    }
                }
                Err(_) => {
                    let _ = durable_unlink(&layout.index());
                    cache_ok = false;
                }
            }
        }
        if !cache_ok {
            let temp_dir = layout
                .cache()
                .join(format!(".recover-{}", clock().uuid4().replace('-', "")));
            crate::durable_fs::create_dir_all_private(&temp_dir)
                .map_err(|err| Error::cache(err.to_string()))?;
            crate::durable_fs::chmod_private(&temp_dir, true)
                .map_err(|err| Error::cache(err.to_string()))?;
            let tmp = temp_dir.join("memory.mcache");
            let result = (|| -> Result<()> {
                cache::populate_index(layout, &tmp, &cards, &invalid, generation, &uuid, &runtime)?;
                cache::validate_cache_file(&tmp, &cards)?;
                crate::durable_fs::replace_durable(&tmp, &layout.index())
                    .map_err(|err| Error::cache(err.to_string()))?;
                Ok(())
            })();
            let _ = std::fs::remove_dir_all(&temp_dir);
            result?;
        }
        if action == "completed-source-and-cache" {
            if let Some((cid, date)) = &legacy_feedback_date {
                let mark = layout.feedback_state().join(format!("{cid}.mark"));
                atomic_write_bytes(&mark, format!("{date}\n").as_bytes())
                    .map_err(|err| Error::core(err.to_string()))?;
            }
        }
        durable_unlink(&journal_path).map_err(|err| Error::core(err.to_string()))?;
        reports.push(crate::jobject! {
            "uuid" => uuid,
            "action" => action,
        });
    }
    Ok(reports)
}
