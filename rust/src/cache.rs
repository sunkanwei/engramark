//! SQLite cache v7: creation, rebuild, validation, incremental sync and the
//! read paths. The cache is a derived artifact; cards/ is the source of truth.
//! v7 keeps v6's business tables and radar logic but the metadata records the
//! cache structure version, exact SQLite version, compile-options hash,
//! capability probe results, Unicode data version, source-set hash, build
//! UUID, generation and completion flag — no Python fingerprint.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, TransactionBehavior};

use crate::anchors::{char_grams, derive_anchors, Anchor};
use crate::clock::clock;
use crate::hash::{semantic_hash, sha256_hex, source_collection_hash_items};
use crate::json::Json;
use crate::lock::FileLock;
use crate::mem::{card_is_current, parse_card, Card};
use crate::normalize::normalize_text;
use crate::paths::Layout;
use crate::pyregex;
use crate::radar::{build_radar_blob, decode_radar_blob};
use crate::textops::MetaRow;
use crate::{
    Error, Result, CACHE_SCHEMA_VERSION, MAX_APPLIED_OPS, MAX_TRIGRAM_TEXT, MEM_FORMAT_VERSION,
    NORMALIZATION_VERSION, QUERY_PLANNER_VERSION, RADAR_COMPILER_VERSION,
    SOURCE_COLLECTION_HASH_VERSION,
};

pub const QUERY_TIMEOUT_MS_DEFAULT: i64 = 500;

#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub fingerprint: Json,
}

fn sqlite_version_tuple() -> (u64, u64, u64) {
    let mut parts = rusqlite::version().split('.');
    let next = |parts: &mut std::str::Split<'_, char>| {
        parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0)
    };
    (next(&mut parts), next(&mut parts), next(&mut parts))
}

fn sqlite_has_wal_reset_fix(version: (u64, u64, u64)) -> bool {
    version >= (3, 51, 3)
        || ((3, 50, 7)..(3, 51, 0)).contains(&version)
        || ((3, 44, 6)..(3, 45, 0)).contains(&version)
}

fn probe_sqlite_capabilities() -> bool {
    let Ok(conn) = Connection::open_in_memory() else {
        return false;
    };
    conn.execute("CREATE TABLE s(x INTEGER) STRICT", [])
        .and_then(|_| {
            conn.execute(
                "CREATE VIRTUAL TABLE u USING fts5(x, tokenize='unicode61')",
                [],
            )
        })
        .and_then(|_| {
            conn.execute(
                "CREATE VIRTUAL TABLE t USING fts5(x, tokenize='trigram')",
                [],
            )
        })
        .and_then(|_| {
            conn.execute(
                "CREATE VIRTUAL TABLE d USING fts5(x, h UNINDEXED, content='', \
                 contentless_delete=1, contentless_unindexed=1)",
                [],
            )
        })
        .is_ok()
}

/// The v7 capability probe: STRICT, FTS5, trigram, contentless-delete and
/// contentless-unindexed must all work in the shipped binary, and the bundled
/// SQLite must carry the WAL-reset fix.
pub fn probe_runtime() -> Result<RuntimeInfo> {
    if !probe_sqlite_capabilities() {
        return Err(Error::core(
            "固定 SQLite 缺少 STRICT/FTS5/trigram/contentless-delete 能力",
        ));
    }
    let version = sqlite_version_tuple();
    if !sqlite_has_wal_reset_fix(version) {
        return Err(Error::core(format!(
            "SQLite {} 不含 WAL-reset 修复",
            rusqlite::version()
        )));
    }
    let conn = Connection::open_in_memory()
        .map_err(|err| Error::core(format!("无法打开内存 SQLite：{err}")))?;
    let mut stmt = conn
        .prepare("PRAGMA compile_options")
        .map_err(|err| Error::core(err.to_string()))?;
    let mut options: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| Error::core(err.to_string()))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|err| Error::core(err.to_string()))?;
    options.sort();
    let compile_options_sha256 = sha256_hex(options.join("\n").as_bytes());
    let features = crate::jobject! {
        "fts5" => true,
        "trigram" => true,
        "strict" => true,
        "contentless_delete" => true,
        "contentless_unindexed" => true,
    };
    let fingerprint = crate::jobject! {
        "fingerprint_format" => 3i64,
        "sqlite_version" => rusqlite::version(),
        "compile_options_sha256" => compile_options_sha256,
        "features" => features,
        "unicode_data_version" => format!(
            "{}.{}.{}",
            crate::casefold_table::UNICODE_DATA_VERSION.0,
            crate::casefold_table::UNICODE_DATA_VERSION.1,
            crate::casefold_table::UNICODE_DATA_VERSION.2
        ),
    };
    Ok(RuntimeInfo { fingerprint })
}

fn pragma_set(conn: &Connection, statement: &str) -> rusqlite::Result<()> {
    // journal_mode/synchronous return a row; most others return none.
    match conn.query_row(statement, [], |_| Ok(())) {
        Ok(()) => Ok(()),
        Err(_) => conn.execute(statement, []).map(|_| ()),
    }
}

pub fn open_write(path: &Path, busy_timeout: Duration) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(busy_timeout)?;
    apply_write_pragmas(&conn)?;
    Ok(conn)
}

pub fn open_readonly(path: &Path, busy_timeout: Duration) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(busy_timeout)?;
    pragma_set(&conn, "PRAGMA query_only=ON")?;
    pragma_set(&conn, "PRAGMA trusted_schema=OFF")?;
    pragma_set(&conn, "PRAGMA mmap_size=0")?;
    Ok(conn)
}

pub fn apply_write_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    let journal_mode: String =
        conn.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "journal_mode={journal_mode}"
        )));
    }
    pragma_set(conn, "PRAGMA synchronous=FULL")?;
    pragma_set(conn, "PRAGMA foreign_keys=ON")?;
    pragma_set(conn, "PRAGMA trusted_schema=OFF")?;
    pragma_set(conn, "PRAGMA mmap_size=0")?;
    #[cfg(target_os = "macos")]
    pragma_set(conn, "PRAGMA fullfsync=ON")?;
    Ok(())
}

thread_local! {
    static QUERY_DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

fn progress_fn() -> bool {
    QUERY_DEADLINE.with(|cell| {
        cell.get()
            .is_some_and(|deadline| Instant::now() >= deadline)
    })
}

/// Install a monotonic-clock progress handler; SQLITE_INTERRUPT is mapped by
/// the caller to the frozen time-budget error.
pub fn set_query_deadline(conn: &Connection, deadline: Option<Instant>) {
    QUERY_DEADLINE.with(|cell| cell.set(deadline));
    if deadline.is_some() {
        let _ = conn.progress_handler(1000, Some(progress_fn));
    } else {
        let _ = conn.progress_handler(0, None::<fn() -> bool>);
    }
}

pub fn is_interrupt(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ErrorCode::OperationInterrupted
    )
}

pub fn load_card_file(path: &Path) -> Result<Card> {
    if crate::paths::is_link_like(path) {
        return Err(Error::core(format!(
            "卡片文件不能是符号链接：{}",
            file_name(path)
        )));
    }
    let raw = std::fs::read(path).map_err(|err| Error::core(format!("无法读取卡片：{err}")))?;
    let text = String::from_utf8(raw)
        .map_err(|_| Error::core(format!("{} 不是有效 UTF-8", file_name(path))))?;
    let card = parse_card(&text)?;
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem.bytes().all(|b| b.is_ascii_digit()) && !stem.is_empty() {
            if let Ok(file_id) = stem.parse::<i64>() {
                if file_id != card.id {
                    return Err(Error::core(format!(
                        "文件名 id {stem} 与卡头 @{} 不一致",
                        card.id
                    )));
                }
            }
        }
    }
    Ok(card)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn all_cards(layout: &Layout) -> Vec<Card> {
    let mut out = Vec::new();
    let dir = layout.cards();
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "mem"))
            .collect(),
        Err(_) => return out,
    };
    entries.sort();
    for path in entries {
        match load_card_file(&path) {
            Ok(card) => out.push(card),
            Err(err) => {
                crate::lifecycle::log(layout, &format!("禁用坏卡 {}: {err}", path.display()))
            }
        }
    }
    out
}

pub fn source_hash(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(data) => sha256_hex(&data),
        Err(_) => String::new(),
    }
}

fn sorted_mem_files(dir: &Path) -> Vec<PathBuf> {
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

/// _load_cards_for_cache: parse all cards, collect invalid ones, verify
/// duplicate ids, supersedes references and cycles.
pub fn load_cards_for_cache(layout: &Layout) -> Result<(Vec<Card>, Vec<Json>)> {
    let mut cards = Vec::new();
    let mut invalid: Vec<Json> = Vec::new();
    for path in sorted_mem_files(&layout.cards()) {
        match load_card_file(&path) {
            Ok(card) => cards.push(card),
            Err(err) => invalid.push(crate::jobject! {
                "path" => relative_path(layout, &path),
                "error" => err.to_string(),
            }),
        }
    }
    let ids: HashSet<i64> = cards.iter().map(|card| card.id).collect();
    if ids.len() != cards.len() {
        return Err(Error::core("存在重复卡片编号"));
    }
    for card in &cards {
        let missing: Vec<i64> = card
            .supersedes
            .iter()
            .filter(|cid| !ids.contains(cid))
            .copied()
            .collect();
        if !missing.is_empty() {
            let mut missing = missing;
            missing.sort_unstable();
            missing.dedup();
            invalid.push(crate::jobject! {
                "path" => relative_path(layout, &layout.card_path(card.id)),
                "error" => format!("引用不存在的卡片：{missing:?}"),
            });
        }
    }
    // Cycle detection with the same visit order as Python (dict insertion).
    let graph: HashMap<i64, &Card> = cards.iter().map(|card| (card.id, card)).collect();
    let mut marks: HashMap<i64, Mark> = HashMap::new();
    for card in &cards {
        visit(card.id, &graph, &mut marks)?;
    }
    Ok((cards, invalid))
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Visiting,
    Visited,
}

fn visit(cid: i64, graph: &HashMap<i64, &Card>, marks: &mut HashMap<i64, Mark>) -> Result<()> {
    match marks.get(&cid) {
        Some(Mark::Visiting) => return Err(Error::core(format!("supersedes 形成环，涉及 @{cid}"))),
        Some(Mark::Visited) => return Ok(()),
        None => {}
    }
    marks.insert(cid, Mark::Visiting);
    if let Some(card) = graph.get(&cid) {
        for next in &card.supersedes {
            visit(*next, graph, marks)?;
        }
    }
    marks.insert(cid, Mark::Visited);
    Ok(())
}

pub fn relative_path(layout: &Layout, path: &Path) -> String {
    path.strip_prefix(&layout.home)
        .ok()
        .and_then(Path::to_str)
        .map(|rel| rel.replace('\\', "/"))
        .expect("validated Engramark source path must be Unicode and inside its data root")
}

fn source_collection_hash(layout: &Layout) -> String {
    let items: Vec<(String, Vec<u8>)> = sorted_mem_files(&layout.cards())
        .iter()
        .filter_map(|path| {
            std::fs::read(path)
                .ok()
                .map(|data| (relative_path(layout, path), data))
        })
        .collect();
    source_collection_hash_items(&items)
}

fn effective_ids(cards: &[Card]) -> HashSet<i64> {
    let superseded: HashSet<i64> = cards
        .iter()
        .filter(|card| card.status == "published" && card_is_current(card))
        .flat_map(|card| card.supersedes.iter().copied())
        .collect();
    cards
        .iter()
        .filter(|card| {
            (card.status == "candidate" && card_is_current(card))
                || (card.status == "published"
                    && card_is_current(card)
                    && !superseded.contains(&card.id))
        })
        .map(|card| card.id)
        .collect()
}

fn trigram_text(card: &Card, anchors: &[Anchor]) -> String {
    let text = [card.title.as_str()]
        .into_iter()
        .chain(card.body.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let urls = pyregex::find_urls(&text);
    let identifiers: Vec<&str> = anchors
        .iter()
        .filter(|anchor| {
            matches!(
                anchor.kind.as_str(),
                "url" | "domain" | "path" | "identifier" | "manual"
            )
        })
        .map(|anchor| anchor.value.as_str())
        .collect();
    let mut value = card.title.clone();
    for entity in &card.entities {
        value.push('\n');
        value.push_str(entity);
    }
    for identifier in identifiers {
        value.push('\n');
        value.push_str(identifier);
    }
    for (start, end) in urls {
        value.push('\n');
        value.push_str(&text[start..end]);
    }
    // Truncate to MAX_TRIGRAM_TEXT bytes without splitting a code point.
    if value.len() <= MAX_TRIGRAM_TEXT {
        return value;
    }
    let mut end = MAX_TRIGRAM_TEXT;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn cache_meta_rows(
    layout: &Layout,
    generation: i64,
    build_uuid: &str,
    invalid: &[Json],
    runtime: &RuntimeInfo,
) -> Vec<(String, String)> {
    vec![
        ("mem_format_version".into(), MEM_FORMAT_VERSION.to_string()),
        (
            "cache_schema_version".into(),
            CACHE_SCHEMA_VERSION.to_string(),
        ),
        (
            "query_planner_version".into(),
            QUERY_PLANNER_VERSION.to_string(),
        ),
        (
            "normalization_version".into(),
            NORMALIZATION_VERSION.to_string(),
        ),
        (
            "tokenizer_version".into(),
            crate::TOKENIZER_VERSION.to_string(),
        ),
        (
            "radar_compiler_version".into(),
            RADAR_COMPILER_VERSION.to_string(),
        ),
        (
            "sqlite_capability_fingerprint".into(),
            runtime.fingerprint.dumps_canonical(),
        ),
        (
            "source_collection_hash_version".into(),
            SOURCE_COLLECTION_HASH_VERSION.to_string(),
        ),
        (
            "source_collection_hash".into(),
            source_collection_hash(layout),
        ),
        ("generation".into(), generation.to_string()),
        ("build_uuid".into(), build_uuid.to_string()),
        ("build_complete".into(), "1".into()),
        ("effective_date".into(), clock().today()),
        (
            "invalid_cards".into(),
            Json::Array(invalid.to_vec()).dumps_canonical(),
        ),
    ]
}

const META_DDL: &str = "CREATE TABLE meta(\
    id INTEGER PRIMARY KEY, \
    status TEXT NOT NULL CHECK(status IN ('candidate','published','archived','tombstone')),\
    type TEXT NOT NULL CHECK(type IN ('fact','decision','skill')),\
    i INTEGER NOT NULL CHECK(i BETWEEN 0 AND 3),\
    t INTEGER NOT NULL CHECK(t BETWEEN 0 AND 6),\
    last_used TEXT NOT NULL, updated TEXT NOT NULL, source TEXT NOT NULL,\
    lock INTEGER NOT NULL CHECK(lock IN (0,1)), scope TEXT NOT NULL,\
    title TEXT NOT NULL, body TEXT NOT NULL, entities TEXT NOT NULL,\
    valid_from TEXT NOT NULL, valid_to TEXT NOT NULL, supersedes TEXT NOT NULL,\
    semantic_hash TEXT NOT NULL CHECK(length(semantic_hash)=64),\
    source_hash TEXT NOT NULL CHECK(length(source_hash)=64)) STRICT";

fn create_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "{META_DDL};
         CREATE TABLE entities(card_id INTEGER NOT NULL REFERENCES meta(id) ON DELETE CASCADE,\
             position INTEGER NOT NULL, value TEXT NOT NULL, norm TEXT NOT NULL,\
             PRIMARY KEY(card_id,norm), UNIQUE(card_id,position)) STRICT;
         CREATE TABLE anchors(card_id INTEGER NOT NULL REFERENCES meta(id) ON DELETE CASCADE,\
             value TEXT NOT NULL, norm TEXT NOT NULL, kind TEXT NOT NULL,\
             strength TEXT NOT NULL CHECK(strength IN ('strong','weak')),\
             manual INTEGER NOT NULL CHECK(manual IN (0,1)),\
             PRIMARY KEY(card_id,norm,kind)) STRICT;
         CREATE INDEX anchors_norm_idx ON anchors(norm);
         CREATE TABLE anchor_grams(gram TEXT NOT NULL, card_id INTEGER NOT NULL \
             REFERENCES meta(id) ON DELETE CASCADE,norm TEXT NOT NULL,\
             PRIMARY KEY(gram,card_id,norm)) STRICT;
         CREATE TABLE cache_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) STRICT;
         CREATE TABLE applied_ops(uuid TEXT PRIMARY KEY,applied_at TEXT NOT NULL) STRICT;
         CREATE VIRTUAL TABLE fts USING fts5(title,body,entities,anchors,\
             semantic_hash UNINDEXED,content='',contentless_delete=1,contentless_unindexed=1,\
             tokenize='unicode61 remove_diacritics 2');
         CREATE VIRTUAL TABLE fts_tri USING fts5(title,tokens,semantic_hash UNINDEXED,\
             content='',contentless_delete=1,contentless_unindexed=1,tokenize='trigram');
         CREATE TABLE radar_cache(generation INTEGER PRIMARY KEY,blob BLOB NOT NULL) STRICT;"
    ))
}

fn insert_card_rows(
    conn: &Connection,
    layout: &Layout,
    card: &Card,
    cfg: &Json,
) -> Result<Vec<Anchor>> {
    let anchors = derive_anchors(card, cfg);
    let body = card.body.join("\n");
    let entities = crate::mem::canonical_entities(&card.entities);
    conn.execute(
        "INSERT INTO meta VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            card.id,
            card.status,
            card.card_type,
            card.importance,
            card.trust,
            card.last_used,
            card.updated,
            card.source,
            if card.lock { 1 } else { 0 },
            card.scope,
            card.title,
            body,
            entities.join("\n"),
            card.valid_from,
            card.valid_to,
            join_supersedes(&card.supersedes),
            semantic_hash(card),
            source_hash(&layout.card_path(card.id)),
        ],
    )
    .map_err(|err| Error::cache(format!("写入 meta 失败：{err}")))?;
    {
        let mut stmt = conn
            .prepare("INSERT INTO entities VALUES(?,?,?,?)")
            .map_err(|err| Error::cache(err.to_string()))?;
        for (position, value) in entities.iter().enumerate() {
            stmt.execute(params![
                card.id,
                position as i64,
                value,
                normalize_text(value)
            ])
            .map_err(|err| Error::cache(err.to_string()))?;
        }
    }
    let mut anchor_stmt = conn
        .prepare("INSERT INTO anchors VALUES(?,?,?,?,?,?)")
        .map_err(|err| Error::cache(err.to_string()))?;
    let mut gram_stmt = conn
        .prepare("INSERT OR IGNORE INTO anchor_grams VALUES(?,?,?)")
        .map_err(|err| Error::cache(err.to_string()))?;
    for anchor in &anchors {
        anchor_stmt
            .execute(params![
                card.id,
                anchor.value,
                anchor.norm,
                anchor.kind,
                anchor.strength(),
                if anchor.manual { 1 } else { 0 },
            ])
            .map_err(|err| Error::cache(err.to_string()))?;
        let norm_len = crate::normalize::py_len(&anchor.norm);
        if (4..=256).contains(&norm_len) {
            for gram in char_grams(&anchor.norm) {
                gram_stmt
                    .execute(params![gram, card.id, anchor.norm])
                    .map_err(|err| Error::cache(err.to_string()))?;
            }
        }
    }
    Ok(anchors)
}

fn join_supersedes(supersedes: &[i64]) -> String {
    let mut sorted = supersedes.to_vec();
    sorted.sort_unstable();
    sorted
        .iter()
        .map(|cid| cid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn insert_fts_rows(conn: &Connection, card: &Card, anchors: &[Anchor], sem: &str) -> Result<()> {
    let body = card.body.join("\n");
    let entities = crate::mem::canonical_entities(&card.entities).join("\n");
    let anchor_text = anchors
        .iter()
        .map(|anchor| anchor.value.clone())
        .collect::<Vec<_>>()
        .join("\n");
    conn.execute(
        "INSERT INTO fts(rowid,title,body,entities,anchors,semantic_hash) VALUES(?,?,?,?,?,?)",
        params![card.id, card.title, body, entities, anchor_text, sem],
    )
    .map_err(|err| Error::cache(err.to_string()))?;
    conn.execute(
        "INSERT INTO fts_tri(rowid,title,tokens,semantic_hash) VALUES(?,?,?,?)",
        params![card.id, card.title, trigram_text(card, anchors), sem],
    )
    .map_err(|err| Error::cache(err.to_string()))?;
    Ok(())
}

pub fn populate_index(
    layout: &Layout,
    path: &Path,
    cards: &[Card],
    invalid: &[Json],
    generation: i64,
    build_uuid: &str,
    runtime: &RuntimeInfo,
) -> Result<()> {
    let cfg = crate::config::load_config(&layout.home);
    let mut conn = Connection::open(path).map_err(|err| Error::cache(err.to_string()))?;
    apply_write_pragmas(&conn).map_err(|err| Error::cache(err.to_string()))?;
    // Rebuild is a single atomic database operation. Without an explicit
    // transaction SQLite autocommits and FULL-syncs every inserted row, which
    // makes a 10k-card rebuild take minutes instead of seconds.
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|err| Error::cache(err.to_string()))?;
    create_schema(&transaction).map_err(|err| Error::cache(err.to_string()))?;
    let effective = effective_ids(cards);
    for card in cards {
        let anchors = insert_card_rows(&transaction, layout, card, &cfg)?;
        if effective.contains(&card.id) {
            insert_fts_rows(&transaction, card, &anchors, &semantic_hash(card))?;
        }
    }
    let blob = build_radar_blob(cards, &cfg);
    transaction
        .execute(
            "INSERT INTO radar_cache VALUES(?,?)",
            params![generation, blob],
        )
        .map_err(|err| Error::cache(err.to_string()))?;
    {
        let mut stmt = transaction
            .prepare("INSERT INTO cache_meta VALUES(?,?)")
            .map_err(|err| Error::cache(err.to_string()))?;
        for (key, value) in cache_meta_rows(layout, generation, build_uuid, invalid, runtime) {
            stmt.execute(params![key, value])
                .map_err(|err| Error::cache(err.to_string()))?;
        }
    }
    transaction
        .commit()
        .map_err(|err| Error::cache(err.to_string()))?;
    Ok(())
}

/// Incremental sync after source changes; the commit is the linearization
/// point of the API write.
pub fn sync_cache(
    conn: &Connection,
    layout: &Layout,
    cards: &[Card],
    invalid: &[Json],
    generation: i64,
    op_uuid: &str,
    runtime: &RuntimeInfo,
) -> Result<()> {
    let cfg = crate::config::load_config(&layout.home);
    let by_id: BTreeMap<i64, &Card> = cards.iter().map(|card| (card.id, card)).collect();
    let desired = effective_ids(cards);
    let result = (|| -> Result<()> {
        conn.execute("BEGIN IMMEDIATE", [])
            .map_err(|err| Error::cache(err.to_string()))?;
        let inner = (|| -> Result<()> {
            let mut current_meta: HashMap<i64, (String, String)> = HashMap::new();
            {
                let mut stmt = conn
                    .prepare("SELECT id,semantic_hash,source_hash FROM meta")
                    .map_err(|err| Error::cache(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                        ))
                    })
                    .map_err(|err| Error::cache(err.to_string()))?;
                for row in rows {
                    let (id, hashes) = row.map_err(|err| Error::cache(err.to_string()))?;
                    current_meta.insert(id, hashes);
                }
            }
            let mut fts_state: HashMap<&str, HashMap<i64, String>> = HashMap::new();
            for table in ["fts", "fts_tri"] {
                let mut state = HashMap::new();
                let mut stmt = conn
                    .prepare(&format!("SELECT rowid,semantic_hash FROM {table}"))
                    .map_err(|err| Error::cache(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| Error::cache(err.to_string()))?;
                for row in rows {
                    let (id, sem) = row.map_err(|err| Error::cache(err.to_string()))?;
                    state.insert(id, sem);
                }
                fts_state.insert(table, state);
            }
            let removed: BTreeSet<i64> = current_meta
                .keys()
                .filter(|cid| !by_id.contains_key(cid))
                .copied()
                .collect();
            let mut changed: BTreeSet<i64> = BTreeSet::new();
            for (cid, card) in &by_id {
                let state = (semantic_hash(card), source_hash(&layout.card_path(*cid)));
                if current_meta.get(cid) != Some(&state) {
                    changed.insert(*cid);
                }
            }
            for cid in removed.union(&changed).copied().collect::<BTreeSet<_>>() {
                for table in ["fts", "fts_tri"] {
                    if fts_state[table].contains_key(&cid) {
                        conn.execute(&format!("DELETE FROM {table} WHERE rowid=?"), params![cid])
                            .map_err(|err| Error::cache(err.to_string()))?;
                        fts_state
                            .get_mut(table)
                            .and_then(|state| state.remove(&cid));
                    }
                }
                conn.execute("DELETE FROM meta WHERE id=?", params![cid])
                    .map_err(|err| Error::cache(err.to_string()))?;
            }
            let mut anchors_by_id: HashMap<i64, Vec<Anchor>> = HashMap::new();
            for cid in &changed {
                anchors_by_id.insert(*cid, insert_card_rows(conn, layout, by_id[cid], &cfg)?);
            }
            for (cid, card) in &by_id {
                let sem = semantic_hash(card);
                let should_exist = desired.contains(cid);
                let needs_anchors = should_exist
                    && ["fts", "fts_tri"]
                        .iter()
                        .any(|table| fts_state[table].get(cid) != Some(&sem));
                let anchors = match anchors_by_id.get(cid) {
                    Some(anchors) => Some(anchors.clone()),
                    None if needs_anchors => Some(derive_anchors(card, &cfg)),
                    None => None,
                };
                for table in ["fts", "fts_tri"] {
                    let mut exists_hash = fts_state[table].get(cid).cloned();
                    if exists_hash.is_some()
                        && (!should_exist || exists_hash.as_deref() != Some(sem.as_str()))
                    {
                        conn.execute(&format!("DELETE FROM {table} WHERE rowid=?"), params![cid])
                            .map_err(|err| Error::cache(err.to_string()))?;
                        exists_hash = None;
                    }
                    if should_exist && exists_hash.is_none() {
                        let empty = Vec::new();
                        let anchors = anchors.as_ref().unwrap_or(&empty);
                        insert_fts_row_into(conn, table, card, anchors, &sem)?;
                    }
                }
            }
            conn.execute("DELETE FROM radar_cache", [])
                .map_err(|err| Error::cache(err.to_string()))?;
            conn.execute(
                "INSERT INTO radar_cache VALUES(?,?)",
                params![generation, build_radar_blob(cards, &cfg)],
            )
            .map_err(|err| Error::cache(err.to_string()))?;
            conn.execute("DELETE FROM cache_meta", [])
                .map_err(|err| Error::cache(err.to_string()))?;
            {
                let mut stmt = conn
                    .prepare("INSERT INTO cache_meta VALUES(?,?)")
                    .map_err(|err| Error::cache(err.to_string()))?;
                for (key, value) in cache_meta_rows(layout, generation, op_uuid, invalid, runtime) {
                    stmt.execute(params![key, value])
                        .map_err(|err| Error::cache(err.to_string()))?;
                }
            }
            conn.execute(
                "INSERT OR IGNORE INTO applied_ops VALUES(?,?)",
                params![op_uuid, clock().isoformat_seconds()],
            )
            .map_err(|err| Error::cache(err.to_string()))?;
            conn.execute(
                "DELETE FROM applied_ops WHERE rowid NOT IN \
                 (SELECT rowid FROM applied_ops ORDER BY rowid DESC LIMIT ?)",
                params![MAX_APPLIED_OPS],
            )
            .map_err(|err| Error::cache(err.to_string()))?;
            let expected: BTreeMap<i64, String> = cards
                .iter()
                .filter(|card| desired.contains(&card.id))
                .map(|card| (card.id, semantic_hash(card)))
                .collect();
            for table in ["fts", "fts_tri"] {
                let mut stmt = conn
                    .prepare(&format!("SELECT rowid,semantic_hash FROM {table}"))
                    .map_err(|err| Error::cache(err.to_string()))?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|err| Error::cache(err.to_string()))?;
                let mut actual = BTreeMap::new();
                let mut count = 0usize;
                for row in rows {
                    let (id, sem) = row.map_err(|err| Error::cache(err.to_string()))?;
                    count += 1;
                    actual.insert(id, sem);
                }
                if count != actual.len() || actual != expected {
                    return Err(Error::cache(format!("增量更新后 {table} 业务校验失败")));
                }
            }
            Ok(())
        })();
        match inner {
            Ok(()) => {
                conn.execute("COMMIT", [])
                    .map_err(|err| Error::cache(err.to_string()))?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(err)
            }
        }
    })();
    result
}

fn insert_fts_row_into(
    conn: &Connection,
    table: &str,
    card: &Card,
    anchors: &[Anchor],
    sem: &str,
) -> Result<()> {
    let body = card.body.join("\n");
    let entities = crate::mem::canonical_entities(&card.entities).join("\n");
    if table == "fts" {
        let anchor_text = anchors
            .iter()
            .map(|anchor| anchor.value.clone())
            .collect::<Vec<_>>()
            .join("\n");
        conn.execute(
            "INSERT INTO fts(rowid,title,body,entities,anchors,semantic_hash) VALUES(?,?,?,?,?,?)",
            params![card.id, card.title, body, entities, anchor_text, sem],
        )
        .map_err(|err| Error::cache(err.to_string()))?;
    } else {
        conn.execute(
            "INSERT INTO fts_tri(rowid,title,tokens,semantic_hash) VALUES(?,?,?,?)",
            params![card.id, card.title, trigram_text(card, anchors), sem],
        )
        .map_err(|err| Error::cache(err.to_string()))?;
    }
    Ok(())
}

pub fn read_generation_unlocked(layout: &Layout) -> i64 {
    if !layout.index().exists() {
        return 0;
    }
    let Ok(conn) = open_readonly(&layout.index(), Duration::from_secs(5)) else {
        return 0;
    };
    conn.query_row(
        "SELECT value FROM cache_meta WHERE key='generation'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| value.parse::<i64>().ok())
    .unwrap_or(0)
}

pub fn validate_cache_file(path: &Path, cards: &[Card]) -> Result<()> {
    let conn =
        open_write(path, Duration::from_secs(5)).map_err(|err| Error::cache(err.to_string()))?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|err| Error::cache(err.to_string()))?;
    if check != "ok" {
        return Err(Error::cache("SQLite quick_check 失败"));
    }
    conn.execute("INSERT INTO fts(fts) VALUES('integrity-check')", [])
        .map_err(|err| Error::cache(err.to_string()))?;
    conn.execute("INSERT INTO fts_tri(fts_tri) VALUES('integrity-check')", [])
        .map_err(|err| Error::cache(err.to_string()))?;
    let effective = effective_ids(cards);
    let expected: BTreeMap<i64, String> = cards
        .iter()
        .filter(|card| effective.contains(&card.id))
        .map(|card| (card.id, semantic_hash(card)))
        .collect();
    for table in ["fts", "fts_tri"] {
        let mut stmt = conn
            .prepare(&format!("SELECT rowid,semantic_hash FROM {table}"))
            .map_err(|err| Error::cache(err.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|err| Error::cache(err.to_string()))?;
        let mut actual = BTreeMap::new();
        let mut count = 0usize;
        for row in rows {
            let (id, sem) = row.map_err(|err| Error::cache(err.to_string()))?;
            count += 1;
            actual.insert(id, sem);
        }
        if count != actual.len() || actual != expected {
            return Err(Error::cache(format!(
                "{table} 与有效卡片集合或语义哈希不一致"
            )));
        }
    }
    let complete: Option<String> = conn
        .query_row(
            "SELECT value FROM cache_meta WHERE key='build_complete'",
            [],
            |row| row.get(0),
        )
        .ok();
    if complete.as_deref() != Some("1") {
        return Err(Error::cache("缓存构建未完成"));
    }
    let radar: Option<Vec<u8>> = conn
        .query_row(
            "SELECT blob FROM radar_cache ORDER BY generation DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(blob) = radar else {
        return Err(Error::cache("缓存缺少雷达对象"));
    };
    decode_radar_blob(&blob)?;
    Ok(())
}

pub fn expected_cache_meta(runtime: &RuntimeInfo) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("mem_format_version".into(), MEM_FORMAT_VERSION.to_string()),
        (
            "cache_schema_version".into(),
            CACHE_SCHEMA_VERSION.to_string(),
        ),
        (
            "query_planner_version".into(),
            QUERY_PLANNER_VERSION.to_string(),
        ),
        (
            "normalization_version".into(),
            NORMALIZATION_VERSION.to_string(),
        ),
        (
            "tokenizer_version".into(),
            crate::TOKENIZER_VERSION.to_string(),
        ),
        (
            "radar_compiler_version".into(),
            RADAR_COMPILER_VERSION.to_string(),
        ),
        (
            "source_collection_hash_version".into(),
            SOURCE_COLLECTION_HASH_VERSION.to_string(),
        ),
        ("build_complete".into(), "1".into()),
        ("effective_date".into(), clock().today()),
        (
            "sqlite_capability_fingerprint".into(),
            runtime.fingerprint.dumps_canonical(),
        ),
    ])
}

fn read_cache_meta(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut stmt = conn
        .prepare("SELECT key,value FROM cache_meta")
        .map_err(|err| Error::cache(err.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| Error::cache(err.to_string()))?;
    let mut meta = BTreeMap::new();
    for row in rows {
        let (key, value) = row.map_err(|err| Error::cache(err.to_string()))?;
        meta.insert(key, value);
    }
    Ok(meta)
}

fn rebuild_locked(layout: &Layout, runtime: &RuntimeInfo) -> Result<Json> {
    let (cards, invalid) = load_cards_for_cache(layout)?;
    let generation = {
        let _shared = FileLock::acquire(layout, "cache.swap", true, None)?;
        read_generation_unlocked(layout) + 1
    };
    let build_uuid = clock().uuid4();
    let temp_dir = layout
        .cache()
        .join(format!(".rebuild-{}", build_uuid.replace('-', "")));
    crate::durable_fs::create_dir_all_private(&temp_dir)
        .map_err(|err| Error::cache(err.to_string()))?;
    crate::durable_fs::chmod_private(&temp_dir, true)
        .map_err(|err| Error::cache(err.to_string()))?;
    let tmp = temp_dir.join("memory.mcache");
    let result = (|| -> Result<()> {
        populate_index(
            layout,
            &tmp,
            &cards,
            &invalid,
            generation,
            &build_uuid,
            runtime,
        )?;
        validate_cache_file(&tmp, &cards)?;
        let _swap = FileLock::acquire(layout, "cache.swap", false, None)?;
        for suffix in ["-journal", "-wal", "-shm"] {
            if Path::new(&format!("{}{}", tmp.display(), suffix)).exists() {
                return Err(Error::cache(format!("临时缓存仍有热日志 {suffix}")));
            }
        }
        crate::durable_fs::replace_durable(&tmp, &layout.index())
            .map_err(|err| Error::cache(err.to_string()))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&temp_dir);
    result?;
    if !invalid.is_empty() {
        crate::lifecycle::log(
            layout,
            &format!(
                "缓存已禁用 {} 张坏卡: {}",
                invalid.len(),
                Json::Array(invalid.clone()).dumps()
            ),
        );
    }
    Ok(crate::jobject! {
        "generation" => generation,
        "build_uuid" => build_uuid,
        "cards" => cards.len() as i64,
        "invalid_cards" => Json::Array(invalid),
    })
}

pub fn rebuild(layout: &Layout) -> Result<Json> {
    layout
        .ensure()
        .map_err(|err| Error::core(err.to_string()))?;
    let runtime = probe_runtime()?;
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    rebuild_locked(layout, &runtime)
}

fn cache_needs_prepare(layout: &Layout, runtime: &RuntimeInfo) -> bool {
    if !layout.index().is_file() {
        return true;
    }
    let check = (|| -> Result<bool> {
        let _shared = FileLock::acquire(layout, "cache.swap", true, None)?;
        let conn = open_readonly(&layout.index(), Duration::from_millis(100))
            .map_err(|err| Error::cache(err.to_string()))?;
        let meta = read_cache_meta(&conn)?;
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|err| Error::cache(err.to_string()))?;
        if check != "ok" {
            return Ok(true);
        }
        let radar: Option<Vec<u8>> = conn
            .query_row(
                "SELECT blob FROM radar_cache ORDER BY generation DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .ok();
        let Some(blob) = radar else {
            return Ok(true);
        };
        decode_radar_blob(&blob)?;
        let expected = expected_cache_meta(runtime);
        Ok(expected
            .iter()
            .any(|(key, value)| meta.get(key) != Some(value)))
    })();
    check.unwrap_or(true)
}

pub fn prepare_cache_if_needed(layout: &Layout) -> Result<Json> {
    layout
        .ensure()
        .map_err(|err| Error::core(err.to_string()))?;
    let runtime = probe_runtime()?;
    if !cache_needs_prepare(layout, &runtime) {
        return Ok(crate::jobject! {"prepared" => false});
    }
    let _mutation = FileLock::acquire(layout, "mutation", false, None)?;
    if !cache_needs_prepare(layout, &runtime) {
        return Ok(crate::jobject! {"prepared" => false});
    }
    let report = rebuild_locked(layout, &runtime)?;
    let mut pairs = vec![("prepared".to_string(), Json::Bool(true))];
    if let Json::Object(fields) = report {
        pairs.extend(fields);
    }
    Ok(Json::Object(pairs))
}

pub fn ensure_index(layout: &Layout) -> Result<()> {
    layout
        .ensure()
        .map_err(|err| Error::core(err.to_string()))?;
    let runtime = probe_runtime()?;
    if crate::txn::has_pending_transactions(layout) {
        crate::txn::recover_transactions(layout)?;
    }
    if !layout.index().exists() {
        rebuild(layout)?;
        return Ok(());
    }
    let needs_rebuild = {
        let _shared = FileLock::acquire(layout, "cache.swap", true, None)?;
        match open_readonly(&layout.index(), Duration::from_secs(5)) {
            Ok(conn) => match read_cache_meta(&conn) {
                Ok(meta) => {
                    let expected = expected_cache_meta(&runtime);
                    expected.iter().any(|(k, v)| meta.get(k) != Some(v))
                }
                Err(_) => true,
            },
            Err(_) => true,
        }
    };
    if needs_rebuild {
        match rebuild(layout) {
            Err(err @ (Error::Core(_) | Error::LockTimeout(_) | Error::CacheUnavailable(_))) => {
                return Err(err)
            }
            Err(err) => {
                return Err(Error::cache(format!("缓存不可用且自动重建失败：{err}")));
            }
            Ok(_) => {}
        }
    }
    Ok(())
}

/// Shared-lock read guard over the cache; one generation stays consistent
/// across all queries made through it.
pub struct CacheReader {
    _shared: FileLock,
    pub conn: Connection,
}

pub fn cache_reader(layout: &Layout) -> Result<CacheReader> {
    ensure_index(layout)?;
    let shared = FileLock::acquire(layout, "cache.swap", true, None)?;
    let conn = open_readonly(
        &layout.index(),
        Duration::from_secs_f64(crate::lock::DATABASE_TIMEOUT),
    )
    .map_err(|err| Error::cache(format!("缓存读取失败：{err}")))?;
    let complete: Option<String> = conn
        .query_row(
            "SELECT value FROM cache_meta WHERE key='build_complete'",
            [],
            |row| row.get(0),
        )
        .ok();
    if complete.as_deref() != Some("1") {
        return Err(Error::cache("缓存尚未完成构建"));
    }
    Ok(CacheReader {
        _shared: shared,
        conn,
    })
}

pub fn row_to_meta(row: &rusqlite::Row) -> rusqlite::Result<MetaRow> {
    Ok(MetaRow {
        id: row.get(0)?,
        status: row.get(1)?,
        card_type: row.get(2)?,
        i: row.get(3)?,
        t: row.get(4)?,
        last_used: row.get(5)?,
        updated: row.get(6)?,
        source: row.get(7)?,
        lock: row.get::<_, i64>(8)? != 0,
        scope: row.get(9)?,
        title: row.get(10)?,
        body: row.get(11)?,
        entities: row.get(12)?,
        valid_from: row.get(13)?,
        valid_to: row.get(14)?,
        supersedes: row.get(15)?,
        semantic_hash: row.get(16)?,
        source_hash: row.get(17)?,
        ..MetaRow::default()
    })
}

/// Hook read-only connection pragmas (busy_timeout=50ms like the Python
/// fast path; query_only/trusted_schema/mmap already set by open_readonly).
pub fn pragma_readonly_hook(conn: &Connection) {
    let _ = conn.busy_timeout(Duration::from_millis(50));
}

pub fn source_collection_hash_pub(layout: &Layout) -> String {
    source_collection_hash(layout)
}

pub fn read_cache_meta_pub(conn: &Connection) -> Result<BTreeMap<String, String>> {
    read_cache_meta(conn)
}

pub fn get_meta(layout: &Layout, ids: &[i64]) -> Result<Vec<MetaRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let reader = cache_reader(layout)?;
    let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut stmt = reader
        .conn
        .prepare(&format!("SELECT * FROM meta WHERE id IN ({marks})"))
        .map_err(|err| Error::cache(err.to_string()))?;
    let params: Vec<&dyn rusqlite::ToSql> =
        ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
    let rows = stmt
        .query_map(params.as_slice(), row_to_meta)
        .map_err(|err| Error::cache(err.to_string()))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| Error::cache(err.to_string()))?);
    }
    Ok(out)
}
