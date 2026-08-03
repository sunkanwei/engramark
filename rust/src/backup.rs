//! Snapshots, migration, rollback and diagnostics. Formats and safety
//! boundaries match the frozen Python implementation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cache::{
    self, load_card_file, load_cards_for_cache, probe_runtime, source_collection_hash_pub,
};
use crate::clock::clock;
use crate::durable_fs::{atomic_write, atomic_write_bytes, durable_unlink};
use crate::hash::source_collection_hash_items;
use crate::json::Json;
use crate::lifecycle::{initialize_id_sequence, read_id_sequence};
use crate::lock::FileLock;
use crate::mem::{parse_card, serialize_card, Card};
use crate::paths::Layout;
use crate::txn::{commit_source_changes, recover_transactions};
use crate::{Error, Result, SNAPSHOT_MANIFEST_VERSION, SOURCE_COLLECTION_HASH_VERSION};

pub fn migrate_v1(layout: &Layout) -> Result<Json> {
    layout
        .ensure()
        .map_err(|err| Error::core(err.to_string()))?;
    cache::probe_runtime()?;
    cache::ensure_index(layout)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    initialize_id_sequence(layout)?;
    let stamp = format!(
        "{}-{}",
        clock().isoformat_seconds().replace(['-', 'T', ':'], ""),
        &clock().uuid4().replace('-', "")[..8]
    );
    let backup = layout.migration_backups().join(&stamp);
    let mut writes: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    let mut diffs = String::new();
    let originals = sorted_mem(&layout.cards());
    let legacy = if layout.candidates().exists() {
        sorted_mem(&layout.candidates())
    } else {
        Vec::new()
    };
    for path in originals.iter().chain(&legacy) {
        let mut card = load_card_file(path)?;
        let mut target = path.clone();
        if path.parent() == Some(layout.candidates().as_path()) {
            card.status = "candidate".into();
            target = layout.card_path(card.id);
            if target.exists() {
                return Err(Error::core(format!(
                    "迁移候选 @{} 时 cards/ 中已有同编号卡片",
                    card.id
                )));
            }
        }
        let new = serialize_card(&card).into_bytes();
        let old = std::fs::read(path).map_err(|err| Error::core(err.to_string()))?;
        if target != *path || new != old {
            writes.insert(target.clone(), new.clone());
            diffs.push_str(&unified_diff(
                &String::from_utf8_lossy(&old),
                &String::from_utf8_lossy(&new),
                &crate::cache::relative_path(layout, path),
                &crate::cache::relative_path(layout, &target),
            ));
        }
    }
    if writes.is_empty() && legacy.is_empty() {
        return Ok(crate::jobject! {
            "ok" => true,
            "changed" => 0i64,
            "backup" => "",
        });
    }
    for path in originals.iter().chain(&legacy) {
        let rel = crate::cache::relative_path(layout, path);
        atomic_write_bytes(
            &backup.join(&rel),
            &std::fs::read(path).map_err(|err| Error::core(err.to_string()))?,
        )
        .map_err(|err| Error::core(err.to_string()))?;
    }
    atomic_write(&backup.join("migration.diff"), &diffs)
        .map_err(|err| Error::core(err.to_string()))?;
    let changed = writes.len() as i64;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    commit_source_changes(layout, "migrate-mem-v1", writes)?;
    for path in &legacy {
        durable_unlink(path).map_err(|err| Error::core(err.to_string()))?;
    }
    for old in [
        layout.home.join("index.sqlite"),
        layout.home.join("radar.dict"),
    ] {
        if old.exists() {
            let destination = backup.join("legacy-cache").join(
                old.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            if let Some(parent) = destination.parent() {
                crate::durable_fs::create_dir_all_private(parent)
                    .map_err(|err| Error::core(err.to_string()))?;
                crate::durable_fs::chmod_private(parent, true)
                    .map_err(|err| Error::core(err.to_string()))?;
            }
            std::fs::rename(&old, &destination).map_err(|err| Error::core(err.to_string()))?;
            if let Some(parent) = destination.parent() {
                let _ = crate::durable_fs::fsync_dir(parent);
            }
        }
    }
    for old_dir in [layout.candidates(), layout.home.join("locks")] {
        let _ = std::fs::remove_dir(&old_dir);
    }
    Ok(crate::jobject! {
        "ok" => true,
        "changed" => changed,
        "backup" => crate::paths::require_unicode(&backup)
            .map_err(|err| Error::core(err.to_string()))?,
    })
}

fn sorted_mem(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "mem"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

/// Minimal unified diff (difflib.unified_diff-compatible for our inputs:
/// changed files only, lineterm="\n" lines with keepends).
fn unified_diff(old: &str, new: &str, from: &str, to: &str) -> String {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    if old_lines == new_lines {
        return String::new();
    }
    let mut out = format!("--- {from}\n+++ {to}\n");
    out.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        old_lines.len(),
        new_lines.len()
    ));
    for line in &old_lines {
        out.push('-');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    for line in &new_lines {
        out.push('+');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn backup_snapshot(layout: &Layout, destination: &Path) -> Result<Json> {
    recover_transactions(layout)?;
    let destination_text =
        crate::paths::require_unicode(destination).map_err(|err| Error::core(err.to_string()))?;
    let destination = crate::paths::resolve_lenient(&crate::paths::expand_user(destination_text));
    crate::paths::require_unicode(&destination).map_err(|err| Error::core(err.to_string()))?;
    if destination.exists() {
        return Err(Error::core(format!(
            "备份目标已存在：{}",
            destination.display()
        )));
    }
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let temp = destination.with_file_name(format!(
        "{}.tmp-{}",
        destination
            .file_name()
            .and_then(|name| name.to_str().map(str::to_owned))
            .unwrap_or_default(),
        &clock().uuid4().replace('-', "")[..8]
    ));
    let result = (|| -> Result<()> {
        crate::durable_fs::create_dir_all_private(&temp.join("cards"))
            .map_err(|err| Error::core(err.to_string()))?;
        crate::durable_fs::chmod_private(&temp, true)
            .map_err(|err| Error::core(err.to_string()))?;
        crate::durable_fs::chmod_private(&temp.join("cards"), true)
            .map_err(|err| Error::core(err.to_string()))?;
        for path in sorted_mem(&layout.cards()) {
            atomic_write_bytes(
                &temp.join("cards").join(
                    path.file_name()
                        .and_then(|name| name.to_str().map(str::to_owned))
                        .unwrap_or_default(),
                ),
                &std::fs::read(&path).map_err(|err| Error::core(err.to_string()))?,
            )
            .map_err(|err| Error::core(err.to_string()))?;
        }
        atomic_write_bytes(
            &temp.join("id-sequence"),
            &std::fs::read(layout.id_sequence()).map_err(|err| Error::core(err.to_string()))?,
        )
        .map_err(|err| Error::core(err.to_string()))?;
        let cards_count = sorted_mem(&layout.cards()).len() as i64;
        let manifest = crate::jobject! {
            "version" => SNAPSHOT_MANIFEST_VERSION,
            "created_at" => clock().isoformat_seconds(),
            "cards" => cards_count,
            "source_collection_hash_version" => SOURCE_COLLECTION_HASH_VERSION,
            "source_collection_hash" => source_collection_hash_pub(layout),
        };
        atomic_write(
            &temp.join("manifest.json"),
            &format!("{}\n", manifest.dumps_indent2_sorted()),
        )
        .map_err(|err| Error::core(err.to_string()))?;
        std::fs::rename(&temp, &destination).map_err(|err| Error::core(err.to_string()))?;
        if let Some(parent) = destination.parent() {
            let _ = crate::durable_fs::fsync_dir(parent);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temp);
    }
    result?;
    Ok(crate::jobject! {
        "ok" => true,
        "path" => crate::paths::require_unicode(&destination)
            .map_err(|err| Error::core(err.to_string()))?,
    })
}

fn read_snapshot(layout: &Layout, source: &Path) -> Result<(BTreeMap<i64, Vec<u8>>, i64, Json)> {
    let source_text =
        crate::paths::require_unicode(source).map_err(|err| Error::core(err.to_string()))?;
    let source = crate::paths::resolve_lenient(&crate::paths::expand_user(source_text));
    crate::paths::require_unicode(&source).map_err(|err| Error::core(err.to_string()))?;
    let cards_dir = source.join("cards");
    let manifest_path = source.join("manifest.json");
    let sequence_path = source.join("id-sequence");
    if source == crate::paths::resolve_lenient(&layout.home)
        || crate::paths::resolve_lenient(&cards_dir)
            == crate::paths::resolve_lenient(&layout.cards())
    {
        return Err(Error::core("回滚来源不能是当前正在使用的 Engramark 目录"));
    }
    if !cards_dir.is_dir() || !manifest_path.is_file() || !sequence_path.is_file() {
        return Err(Error::core(
            "快照必须包含 cards/、id-sequence 和 manifest.json",
        ));
    }
    for path in [&cards_dir, &manifest_path, &sequence_path] {
        if crate::paths::is_link_like(path) {
            return Err(Error::core("快照控制文件和 cards/ 不得是符号链接"));
        }
    }
    let manifest = Json::parse(
        &std::fs::read_to_string(&manifest_path)
            .map_err(|_| Error::core("快照清单或编号高水位损坏"))?,
    )
    .map_err(|_| Error::core("快照清单或编号高水位损坏"))?;
    let sequence: i64 = std::fs::read_to_string(&sequence_path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .ok_or_else(|| Error::core("快照清单或编号高水位损坏"))?;
    if manifest.get("version").and_then(Json::as_i64) != Some(SNAPSHOT_MANIFEST_VERSION)
        || manifest
            .get("source_collection_hash_version")
            .and_then(Json::as_i64)
            != Some(SOURCE_COLLECTION_HASH_VERSION)
        || sequence < 0
    {
        return Err(Error::core("快照版本不受支持或编号高水位非法"));
    }
    let mut writes: BTreeMap<i64, Vec<u8>> = BTreeMap::new();
    let mut hash_items: Vec<(String, Vec<u8>)> = Vec::new();
    for path in sorted_mem(&cards_dir) {
        if crate::paths::is_link_like(&path) || !path.is_file() {
            return Err(Error::core(format!(
                "快照卡片不能是符号链接：{}",
                name_of(&path)
            )));
        }
        let data = std::fs::read(&path).map_err(|err| Error::core(err.to_string()))?;
        let text = String::from_utf8(data.clone())
            .map_err(|_| Error::core(format!("快照卡片 {} 不是 UTF-8", name_of(&path))))?;
        let card = parse_card(&text)?;
        let expected_name = format!("{:04}.mem", card.id);
        if name_of(&path) != expected_name || writes.contains_key(&card.id) {
            return Err(Error::core(format!(
                "快照卡片文件名、卡头或编号重复：{}",
                name_of(&path)
            )));
        }
        writes.insert(card.id, data.clone());
        hash_items.push((format!("cards/{}", name_of(&path)), data));
    }
    if manifest.get("cards").and_then(Json::as_i64) != Some(writes.len() as i64) {
        return Err(Error::core("快照清单中的卡片数量不一致"));
    }
    if sequence < writes.keys().max().copied().unwrap_or(0) {
        return Err(Error::core("快照编号高水位低于最大卡片编号"));
    }
    let actual_hash = source_collection_hash_items(&hash_items);
    if manifest
        .get("source_collection_hash")
        .and_then(Json::as_str)
        != Some(actual_hash.as_str())
    {
        return Err(Error::core("快照完整源集合哈希不一致"));
    }
    Ok((writes, sequence, manifest))
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn rollback_snapshot(layout: &Layout, source: &Path, confirmed: bool) -> Result<Json> {
    if !confirmed {
        return Err(Error::core("回滚会替换当前记忆；必须显式传入 --confirm"));
    }
    let (snapshot, snapshot_sequence, manifest) = read_snapshot(layout, source)?;
    cache::ensure_index(layout)?;
    let stamp = format!(
        "{}-{}",
        clock().isoformat_seconds().replace(['-', 'T', ':'], ""),
        &clock().uuid4().replace('-', "")[..8]
    );
    let safety = layout.rollback_backups().join(&stamp);
    backup_snapshot(layout, &safety)?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
    let (current_cards, invalid) = load_cards_for_cache(layout)?;
    if !invalid.is_empty() {
        return Err(Error::core(format!(
            "当前源中有坏卡，回滚已停止；安全备份位于 {}",
            safety.display()
        )));
    }
    let current: BTreeMap<i64, Card> = current_cards
        .into_iter()
        .map(|card| (card.id, card))
        .collect();
    let mut writes: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for (cid, data) in &snapshot {
        writes.insert(layout.card_path(*cid), data.clone());
    }
    for cid in current.keys() {
        if snapshot.contains_key(cid) {
            continue;
        }
        let tombstone = Card {
            id: *cid,
            card_type: "fact".into(),
            status: "tombstone".into(),
            importance: 0,
            trust: 0,
            updated: clock().today(),
            source: "system:rollback".into(),
            title: format!("回滚后移除的记忆 @{cid}。"),
            ..Card::new()
        };
        writes.insert(
            layout.card_path(*cid),
            serialize_card(&tombstone).into_bytes(),
        );
    }
    let high_water = read_id_sequence(layout)?
        .max(snapshot_sequence)
        .max(snapshot.keys().max().copied().unwrap_or(0))
        .max(current.keys().max().copied().unwrap_or(0));
    atomic_write(&layout.id_sequence(), &format!("{high_water}\n"))
        .map_err(|err| Error::core(err.to_string()))?;
    commit_source_changes(layout, "rollback-snapshot", writes)?;
    Ok(crate::jobject! {
        "ok" => true,
        "cards" => snapshot.len() as i64,
        "id_sequence" => high_water,
        "snapshot_created_at" => manifest.get("created_at").and_then(Json::as_str).unwrap_or("").to_string(),
        "safety_backup" => crate::paths::require_unicode(&safety)
            .map_err(|err| Error::core(err.to_string()))?,
    })
}

pub fn diagnose(layout: &Layout, full: bool) -> Result<Json> {
    cache::ensure_index(layout)?;
    let (check, meta, cards, invalid);
    {
        let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
        let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
        let loaded = load_cards_for_cache(layout)?;
        let conn = cache::open_write(&layout.index(), std::time::Duration::from_secs(5))
            .map_err(|err| Error::cache(err.to_string()))?;
        check = conn
            .query_row(
                if full {
                    "PRAGMA integrity_check"
                } else {
                    "PRAGMA quick_check"
                },
                [],
                |row| row.get::<_, String>(0),
            )
            .map_err(|err| Error::cache(err.to_string()))?;
        meta = cache::read_cache_meta_pub(&conn)?;
        conn.execute("INSERT INTO fts(fts) VALUES('integrity-check')", [])
            .map_err(|err| Error::cache(err.to_string()))?;
        conn.execute("INSERT INTO fts_tri(fts_tri) VALUES('integrity-check')", [])
            .map_err(|err| Error::cache(err.to_string()))?;
        let _ = conn.execute("ROLLBACK", []);
        cache::validate_cache_file(&layout.index(), &loaded.0)?;
        cards = loaded.0;
        invalid = loaded.1;
    }
    let sequence = read_id_sequence(layout)?;
    let max_card_id = {
        let mut max_id = 0i64;
        if let Ok(entries) = std::fs::read_dir(layout.cards()) {
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
        max_id
    };
    let source_set_hash = source_collection_hash_pub(layout);
    let state_ok = sequence >= max_card_id;
    let source_hash_ok = meta.get("source_collection_hash") == Some(&source_set_hash);
    let generation = meta
        .get("generation")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let runtime = probe_runtime()?;
    Ok(crate::jobject! {
        "ok" => check == "ok" && invalid.is_empty() && state_ok && source_hash_ok,
        "sqlite_check" => check,
        "cards" => cards.len() as i64,
        "invalid_cards" => Json::Array(invalid),
        "id_sequence" => sequence,
        "max_card_id" => max_card_id,
        "id_sequence_valid" => state_ok,
        "cache_generation" => generation,
        "source_collection_hash" => meta.get("source_collection_hash").cloned().unwrap_or_default(),
        "source_collection_hash_valid" => source_hash_ok,
        "runtime" => runtime.fingerprint,
    })
}
