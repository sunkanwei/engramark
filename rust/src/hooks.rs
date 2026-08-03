//! Codex/OpenCode internal hook protocols: scan_text with cooldown, the
//! hook-fast reserve/commit/cancel protocol, and the two live Codex hook
//! entry points. All failure paths are fail-open.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cache::get_meta;
use crate::cache::{self, expected_cache_meta, probe_runtime};
use crate::clock::clock;
use crate::config;
use crate::durable_fs::atomic_write;
use crate::hash::sha256_hex;
use crate::json::Json;
use crate::lock::FileLock;
use crate::normalize::py_len;
use crate::paths::{project_context_id, Layout};
use crate::radar::{decode_radar_blob, radar_hits_from_runtime, RadarHit};
use crate::textops::{human_radar_line, unsafe_display_character, MetaRow};
use crate::{
    Error, Result, CODEX_BLOCK_PREFIX, HOOK_BLOCK_PREFIX, HOOK_BLOCK_SUFFIX,
    HOOK_FAST_TIMEOUT_SECONDS, HOOK_MAX_BLOCK_BYTES, HOOK_MAX_BUDGET, HOOK_MAX_CANDIDATES,
    HOOK_MAX_INPUT_BYTES, HOOK_MAX_LINE_BYTES, HOOK_MAX_LINE_CODEPOINTS, HOOK_MAX_PROJECT_BYTES,
    HOOK_MAX_SESSION_BYTES, HOOK_MAX_TEXT_CHARS, HOOK_PROTOCOL_VERSION,
    HOOK_RESERVATION_TTL_SECONDS, HOOK_STATE_VERSION, RADAR_GIST_MAX_CODEPOINTS,
    RADAR_STATE_MAX_BYTES, RADAR_STATE_VERSION,
};

fn radar_state_path(layout: &Layout, session: &str) -> PathBuf {
    let safe: String = session
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let safe = if safe.is_empty() { "default" } else { &safe };
    layout
        .cache()
        .join("radar-state")
        .join(format!("{safe}.json"))
}

fn best_hits_by_card(hits: Vec<RadarHit>) -> Vec<(i64, RadarHit)> {
    let mut selected: Vec<(i64, RadarHit)> = Vec::new();
    for hit in hits {
        let quality = (hit.strong, hit.manual);
        match selected.iter_mut().find(|(cid, _)| *cid == hit.id) {
            None => selected.push((hit.id, hit)),
            Some((_, current)) => {
                if quality > (current.strong, current.manual) {
                    *current = hit;
                }
            }
        }
    }
    selected
}

pub fn radar_block_size(lines: &[String], prefix: &str, suffix: &str) -> usize {
    format!("{}{}{}", prefix, lines.join("\n"), suffix).len()
}

pub fn pack_radar_candidates(
    candidates: &[Json],
    budget: i64,
    prefix: &str,
    suffix: &str,
) -> Vec<Json> {
    let mut selected: Vec<Json> = Vec::new();
    for candidate in candidates {
        let Some(line) = candidate.get("line").and_then(Json::as_str) else {
            continue;
        };
        let mut trial: Vec<String> = selected
            .iter()
            .filter_map(|item| item.get("line").and_then(Json::as_str).map(str::to_string))
            .collect();
        trial.push(line.to_string());
        if radar_block_size(&trial, prefix, suffix) > HOOK_MAX_BLOCK_BYTES {
            continue;
        }
        selected.push(candidate.clone());
        if selected.len() as i64 >= budget {
            break;
        }
    }
    selected
}

fn radar_gist_limit(radar_cfg: Option<&Json>) -> i64 {
    config::bounded_int(
        config::get(radar_cfg, "gist_max_codepoints"),
        RADAR_GIST_MAX_CODEPOINTS as i64,
        0,
        RADAR_GIST_MAX_CODEPOINTS as i64,
    )
}

/// _fresh_cooldown: keep finite timestamps within [now-ttl, now].
fn fresh_cooldown(cooldown: Option<&Json>, now_ts: f64, ttl: i64) -> Vec<(String, f64)> {
    let mut fresh = Vec::new();
    let Some(pairs) = cooldown.and_then(Json::as_object) else {
        return fresh;
    };
    for (key, value) in pairs {
        let valid_key = !key.is_empty()
            && key.bytes().all(|b| b.is_ascii_digit())
            && key.len() <= 19
            && key.parse::<i64>().is_ok_and(|id| id > 0);
        if !valid_key {
            continue;
        }
        let Some(timestamp) = value.as_f64() else {
            continue;
        };
        let age = now_ts - timestamp;
        if timestamp.is_finite() && (0.0..=ttl as f64).contains(&age) {
            let canonical = key
                .parse::<i64>()
                .map(|id| id.to_string())
                .unwrap_or_else(|_| key.clone());
            fresh.push((canonical, timestamp));
        }
    }
    fresh
}

fn radar_scan(layout: &Layout, text: &str, project: &str) -> Result<Vec<RadarHit>> {
    let mut anchors_ac = None;
    for attempt in 0..2 {
        let (generation, blob) = {
            let reader = cache::cache_reader(layout)?;
            let mut stmt = reader
                .conn
                .prepare("SELECT generation,blob FROM radar_cache ORDER BY generation DESC LIMIT 1")
                .map_err(|err| Error::cache(err.to_string()))?;
            let row = stmt
                .query_row([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(|_| Error::cache("缓存中没有雷达对象"))?;
            drop(stmt);
            row
        };
        let _ = generation;
        match decode_radar_blob(&blob) {
            Ok((anchors, ac)) => {
                anchors_ac = Some((anchors, ac));
                break;
            }
            Err(err) => {
                if attempt == 1 {
                    return Err(err);
                }
                cache::rebuild(layout)?;
            }
        }
    }
    let Some((anchors, ac)) = anchors_ac else {
        return Err(Error::cache("雷达缓存重建后仍不可用"));
    };
    radar_hits_from_runtime(&anchors, &ac, text, project, None, None)
}

fn meta_to_radar_candidate(meta: &MetaRow, entity: &str, gist_limit: i64) -> Option<Json> {
    let line = human_radar_line(meta, entity, gist_limit);
    if !hook_line_valid(&line) {
        return None;
    }
    Some(crate::jobject! {
        "id" => meta.id,
        "entity" => entity,
        "line" => line,
    })
}

fn metas_for_hits(layout: &Layout, hits: &[(i64, RadarHit)]) -> Result<Vec<(MetaRow, RadarHit)>> {
    let ids: Vec<i64> = hits.iter().map(|(cid, _)| *cid).collect();
    let metas = get_meta(layout, &ids)?;
    let by_id: BTreeMap<i64, MetaRow> = metas.into_iter().map(|m| (m.id, m)).collect();
    let mut out: Vec<(MetaRow, RadarHit)> = hits
        .iter()
        .filter_map(|(cid, hit)| {
            by_id.get(cid).and_then(|meta| {
                if meta.status == "published" && meta.is_current() {
                    Some((meta.clone(), hit.clone()))
                } else {
                    None
                }
            })
        })
        .collect();
    out.sort_by(|(ma, ha), (mb, hb)| {
        let ka = (
            !ha.strong,
            !ha.manual,
            !ma.lock,
            -ma.i,
            -ma.t,
            -meta_freshness(ma),
            -ma.id,
        );
        let kb = (
            !hb.strong,
            !hb.manual,
            !mb.lock,
            -mb.i,
            -mb.t,
            -meta_freshness(mb),
            -mb.id,
        );
        ka.cmp(&kb)
    });
    Ok(out)
}

fn meta_freshness(meta: &MetaRow) -> i64 {
    // rank_key freshness participates reversed; use millibel for ordering.
    (meta.freshness_text().parse::<f64>().unwrap_or(0.0) * 1_000_000.0) as i64
}

/// scan_text: the Codex-facing scan with per-session cooldown.
pub fn scan_text(
    layout: &Layout,
    text: &str,
    session: &str,
    budget: Option<i64>,
    project: &str,
) -> Result<Json> {
    let cfg = config::load_config(&layout.home);
    let radar_cfg = config::section(&cfg, "radar");
    let requested_budget = budget
        .or_else(|| config::get(radar_cfg, "budget").and_then(config::py_int))
        .unwrap_or(3)
        .clamp(0, HOOK_MAX_BUDGET);
    if requested_budget == 0 {
        return Ok(crate::jobject! {
            "lines" => Json::Array(Vec::new()),
            "hits" => Json::Array(Vec::new()),
            "context" => "",
        });
    }
    let gist_limit = radar_gist_limit(radar_cfg);
    let state_path = radar_state_path(layout, session);
    let hits = radar_scan(layout, text, project)?;
    let hit_by_id = best_hits_by_card(hits);
    let metas = metas_for_hits(layout, &hit_by_id)?;
    let mut candidates = Vec::new();
    for (meta, hit) in &metas {
        if let Some(candidate) = meta_to_radar_candidate(meta, &hit.entity, gist_limit) {
            candidates.push(candidate);
        }
    }
    // Cooldown read/decide/write is one transaction under radar-state.
    let _state = FileLock::acquire(layout, "radar-state", false, None)?;
    let mut seen: Vec<(String, f64)> = Vec::new();
    if state_path.is_file() {
        if let Ok(meta) = std::fs::metadata(&state_path) {
            if meta.len() <= RADAR_STATE_MAX_BYTES {
                if let Ok(text) = std::fs::read_to_string(&state_path) {
                    if let Ok(state) = Json::parse(&text) {
                        if state.get("version").and_then(Json::as_i64) == Some(RADAR_STATE_VERSION)
                        {
                            if let Some(cooldown) = state.get("cooldown") {
                                seen = fresh_cooldown(
                                    Some(cooldown),
                                    clock().unix_seconds(),
                                    ttl(radar_cfg),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let now_ts = clock().unix_seconds();
    let fresh: Vec<Json> = candidates
        .into_iter()
        .filter(|candidate| {
            let id = candidate.get("id").and_then(Json::as_i64).unwrap_or(0);
            !seen.iter().any(|(key, _)| key == &id.to_string())
        })
        .collect();
    let selected = pack_radar_candidates(&fresh, requested_budget, CODEX_BLOCK_PREFIX, "");
    if selected.is_empty() {
        return Ok(crate::jobject! {
            "lines" => Json::Array(Vec::new()),
            "hits" => Json::Array(Vec::new()),
            "context" => "",
        });
    }
    for candidate in &selected {
        let id = candidate.get("id").and_then(Json::as_i64).unwrap_or(0);
        seen.retain(|(key, _)| key != &id.to_string());
        seen.push((id.to_string(), now_ts));
    }
    let cap = config::get(radar_cfg, "cooldown_max_entries")
        .and_then(config::py_int)
        .unwrap_or(1024)
        .max(16) as usize;
    if seen.len() > cap {
        seen.sort_by(|a, b| b.1.total_cmp(&a.1));
        seen.truncate(cap);
    }
    let state = crate::jobject! {
        "version" => RADAR_STATE_VERSION,
        "cooldown" => Json::Object(seen.iter().map(|(k, v)| (k.clone(), Json::Float(*v))).collect()),
    };
    if let Some(parent) = state_path.parent() {
        let _ = crate::durable_fs::create_dir_all_private(parent);
    }
    atomic_write(&state_path, &state.dumps_canonical())
        .map_err(|err| Error::core(err.to_string()))?;
    let lines: Vec<String> = selected
        .iter()
        .filter_map(|item| item.get("line").and_then(Json::as_str).map(str::to_string))
        .collect();
    Ok(crate::jobject! {
        "lines" => Json::Array(lines.iter().map(|l| Json::Str(l.clone())).collect()),
        "hits" => Json::Array(selected.iter().map(|item| crate::jobject! {
            "entity" => item.get("entity").and_then(Json::as_str).unwrap_or("").to_string(),
            "id" => item.get("id").and_then(Json::as_i64).unwrap_or(0),
        }).collect()),
        "context" => format!("{}{}", CODEX_BLOCK_PREFIX, lines.join("\n")),
    })
}

/// Test-only timeout override (mirrors the Python monkeypatch points).
fn hook_fast_timeout() -> f64 {
    std::env::var("ENGRAMARK_HOOK_FAST_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|ms| ms / 1000.0)
        .unwrap_or(HOOK_FAST_TIMEOUT_SECONDS)
}

fn hook_reservation_ttl() -> f64 {
    std::env::var("ENGRAMARK_HOOK_RESERVATION_TTL_MS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|ms| ms / 1000.0)
        .unwrap_or(HOOK_RESERVATION_TTL_SECONDS)
}

fn ttl(radar_cfg: Option<&Json>) -> i64 {
    config::get(radar_cfg, "cooldown_ttl_seconds")
        .and_then(config::py_int)
        .unwrap_or(86400)
        .max(60)
}

fn hook_unavailable(reason: &'static str) -> Json {
    crate::jobject! {
        "protocol_version" => HOOK_PROTOCOL_VERSION,
        "status" => "unavailable",
        "items" => Json::Array(Vec::new()),
        "reason" => reason,
    }
}

pub fn read_hook_stdin(required: &[&str]) -> Result<Json> {
    let mut raw = vec![0u8; HOOK_MAX_INPUT_BYTES + 1];
    let mut stdin = std::io::stdin().lock();
    let mut total = 0usize;
    loop {
        let read = stdin
            .read(&mut raw[total..])
            .map_err(|_| Error::HookProtocol("hook input too large".into()))?;
        total += read;
        if read == 0 || total > HOOK_MAX_INPUT_BYTES {
            break;
        }
    }
    if total > HOOK_MAX_INPUT_BYTES {
        return Err(Error::HookProtocol("hook input too large".into()));
    }
    let text = std::str::from_utf8(&raw[..total])
        .map_err(|_| Error::HookProtocol("hook input is not strict UTF-8 JSON".into()))?;
    let payload = Json::parse(text)
        .map_err(|_| Error::HookProtocol("hook input is not strict UTF-8 JSON".into()))?;
    if !payload.is_object() {
        return Err(Error::HookProtocol(
            "hook input fields do not match protocol".into(),
        ));
    }
    let mut keys: Vec<&str> = payload.keys();
    keys.sort_unstable();
    let mut expected = required.to_vec();
    expected.sort_unstable();
    if keys != expected {
        return Err(Error::HookProtocol(
            "hook input fields do not match protocol".into(),
        ));
    }
    if payload.get("protocol_version").and_then(Json::as_i64) != Some(HOOK_PROTOCOL_VERSION) {
        return Err(Error::HookProtocol(
            "hook protocol version is unsupported".into(),
        ));
    }
    Ok(payload)
}

fn validate_hook_scan_payload(payload: &Json) -> Result<()> {
    if payload.get("host").and_then(Json::as_str) != Some("opencode") {
        return Err(Error::HookProtocol("hook host is unsupported".into()));
    }
    let session = payload
        .get("session_id")
        .and_then(Json::as_str)
        .unwrap_or("");
    if session.is_empty() || session.len() > HOOK_MAX_SESSION_BYTES {
        return Err(Error::HookProtocol("hook session id is invalid".into()));
    }
    let project_path = payload
        .get("project_path")
        .and_then(Json::as_str)
        .unwrap_or("");
    if project_path.is_empty()
        || !std::path::Path::new(project_path).is_absolute()
        || project_path.len() > HOOK_MAX_PROJECT_BYTES
    {
        return Err(Error::HookProtocol("hook project path is invalid".into()));
    }
    let text = payload.get("text").and_then(Json::as_str).unwrap_or("");
    if payload.get("text").and_then(Json::as_str).is_none() || py_len(text) > HOOK_MAX_TEXT_CHARS {
        return Err(Error::HookProtocol("hook text is invalid".into()));
    }
    match payload.get("budget") {
        Some(Json::Int(budget)) if (0..=HOOK_MAX_BUDGET).contains(budget) => {}
        _ => return Err(Error::HookProtocol("hook budget is invalid".into())),
    }
    Ok(())
}

fn validate_hook_control_payload(payload: &Json) -> Result<()> {
    if payload.get("host").and_then(Json::as_str) != Some("opencode") {
        return Err(Error::HookProtocol("hook host is unsupported".into()));
    }
    let session_key = payload
        .get("session_key")
        .and_then(Json::as_str)
        .unwrap_or("");
    if session_key.len() != 64
        || !session_key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::HookProtocol("hook session key is invalid".into()));
    }
    let reservation = payload
        .get("reservation_id")
        .and_then(Json::as_str)
        .unwrap_or("");
    if !(20..=128).contains(&reservation.len())
        || !reservation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::HookProtocol("hook reservation id is invalid".into()));
    }
    Ok(())
}

fn hook_session_key(host: &str, session_id: &str) -> String {
    sha256_hex(format!("{host}\0{session_id}").as_bytes())
}

fn hook_state_path(layout: &Layout, session_key: &str) -> PathBuf {
    layout
        .cache()
        .join("radar-state")
        .join(format!("hook-{session_key}.json"))
}

fn deadline_remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(Error::HookDeadlineExceeded);
    }
    Ok(remaining)
}

/// Python hook path mapping: interrupted (or past deadline) → timeout,
/// locked/busy → cache_busy, anything else → cache_corrupt.
fn map_hook_sql_err(err: &rusqlite::Error, deadline: Instant) -> Error {
    let text = err.to_string().to_lowercase();
    if cache::is_interrupt(err) || text.contains("interrupted") || Instant::now() >= deadline {
        return Error::HookUnavailable("timeout");
    }
    if text.contains("locked") || text.contains("busy") {
        return Error::HookUnavailable("cache_busy");
    }
    Error::HookUnavailable("cache_corrupt")
}

fn hook_control_character(text: &str) -> bool {
    text.chars().any(unsafe_display_character)
}

fn hook_line_valid(line: &str) -> bool {
    !line.is_empty()
        && py_len(line) <= HOOK_MAX_LINE_CODEPOINTS
        && line.len() <= HOOK_MAX_LINE_BYTES
        && !line.contains('\n')
        && !line.contains('\r')
        && !hook_control_character(line)
        && !line.contains("[long-term-memory-index:")
        && !line.contains("[/long-term-memory-index]")
}

/// _hook_cache_candidates: strict read-only path with unavailability mapping.
fn hook_cache_candidates(
    layout: &Layout,
    payload: &Json,
    deadline: Instant,
    gist_max_codepoints: i64,
) -> Result<Vec<Json>> {
    if !layout.index().is_file() || !layout.locks().is_dir() {
        return Err(Error::HookUnavailable("cache_missing"));
    }
    let runtime = probe_runtime().map_err(|_| Error::HookUnavailable("cache_incompatible"))?;
    let text = payload.get("text").and_then(Json::as_str).unwrap_or("");
    let project_path = payload
        .get("project_path")
        .and_then(Json::as_str)
        .unwrap_or("");
    let result = (|| -> Result<Vec<Json>> {
        let project = project_context_id(Some(project_path), true, layout);
        deadline_remaining(deadline)?;
        let lock_timeout = deadline_remaining(deadline)?.as_secs_f64().min(0.1);
        let _shared =
            FileLock::acquire(layout, "cache.swap", true, Some(lock_timeout)).map_err(|err| {
                match err {
                    Error::LockTimeout(_) => Error::HookUnavailable("cache_busy"),
                    other => other,
                }
            })?;
        let conn = cache::open_readonly(&layout.index(), Duration::ZERO)
            .map_err(|_| Error::HookUnavailable("cache_corrupt"))?;
        cache::pragma_readonly_hook(&conn);
        cache::set_query_deadline(&conn, Some(deadline));
        let inner = (|| -> Result<Vec<Json>> {
            let meta = cache::read_cache_meta_pub(&conn)?;
            let expected = expected_cache_meta(&runtime);
            let version_keys = [
                "mem_format_version",
                "cache_schema_version",
                "query_planner_version",
                "normalization_version",
                "tokenizer_version",
                "radar_compiler_version",
                "source_collection_hash_version",
                "sqlite_capability_fingerprint",
            ];
            if version_keys
                .iter()
                .any(|key| meta.get(*key) != expected.get(*key))
            {
                return Err(Error::HookUnavailable("cache_incompatible"));
            }
            if meta.get("build_complete").map(String::as_str) != Some("1") {
                return Err(Error::HookUnavailable("cache_corrupt"));
            }
            if meta.get("effective_date").map(String::as_str) != Some(clock().today().as_str()) {
                return Err(Error::HookUnavailable("cache_stale"));
            }
            let blob: Vec<u8> = conn
                .query_row(
                    "SELECT blob FROM radar_cache ORDER BY generation DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| Error::HookUnavailable("cache_corrupt"))?;
            let (anchors, automaton) =
                decode_radar_blob(&blob).map_err(|_| Error::HookUnavailable("cache_corrupt"))?;
            let _ = &blob;
            let hits = radar_hits_from_runtime(
                &anchors,
                &automaton,
                text,
                &project,
                Some(HOOK_MAX_CANDIDATES),
                Some(deadline),
            )
            .map_err(|err| match err {
                Error::HookCandidateOverflow => Error::HookUnavailable("internal"),
                other => other,
            })?;
            let mut ids: Vec<i64> = Vec::new();
            for hit in &hits {
                if !ids.contains(&hit.id) {
                    ids.push(hit.id);
                }
            }
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let marks = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM meta WHERE id IN ({marks})"))
                .map_err(|err| map_hook_sql_err(&err, deadline))?;
            let sql_params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(sql_params.as_slice(), cache::row_to_meta)
                .map_err(|err| map_hook_sql_err(&err, deadline))?;
            let mut metas = Vec::new();
            for row in rows {
                metas.push(row.map_err(|err| map_hook_sql_err(&err, deadline))?);
            }
            deadline_remaining(deadline)?;
            let by_id: BTreeMap<i64, MetaRow> =
                metas.into_iter().map(|meta| (meta.id, meta)).collect();
            let hit_by_id = best_hits_by_card(hits);
            let hit_map: BTreeMap<i64, RadarHit> = hit_by_id.into_iter().collect();
            let mut ordered: Vec<(MetaRow, RadarHit)> = ids
                .iter()
                .filter_map(|cid| {
                    by_id.get(cid).and_then(|meta| {
                        if meta.status == "published" && meta.is_current() {
                            hit_map.get(cid).map(|hit| (meta.clone(), hit.clone()))
                        } else {
                            None
                        }
                    })
                })
                .collect();
            ordered.sort_by(|(ma, ha), (mb, hb)| {
                let ka = (
                    !ha.strong,
                    !ha.manual,
                    !ma.lock,
                    -ma.i,
                    -ma.t,
                    -meta_freshness(ma),
                    -ma.id,
                );
                let kb = (
                    !hb.strong,
                    !hb.manual,
                    !mb.lock,
                    -mb.i,
                    -mb.t,
                    -meta_freshness(mb),
                    -mb.id,
                );
                ka.cmp(&kb)
            });
            let mut candidates = Vec::new();
            for (meta, hit) in &ordered {
                deadline_remaining(deadline)?;
                let line = human_radar_line(meta, &hit.entity, gist_max_codepoints);
                if !hook_line_valid(&line) {
                    continue;
                }
                candidates.push(crate::jobject! {
                    "id" => meta.id,
                    "line" => line,
                });
            }
            Ok(candidates)
        })();
        cache::set_query_deadline(&conn, None);
        inner
    })();
    match result {
        Err(Error::LockTimeout(_)) => Err(Error::HookUnavailable("cache_busy")),
        Err(Error::HookDeadlineExceeded) => Err(Error::HookUnavailable("timeout")),
        Err(err) => Err(err),
        ok => ok,
    }
}

fn empty_hook_state(session_key: &str) -> Json {
    crate::jobject! {
        "version" => HOOK_STATE_VERSION,
        "session_key" => session_key,
        "cooldown" => Json::Object(Vec::new()),
        "reservations" => Json::Object(Vec::new()),
    }
}

fn load_hook_state(path: &std::path::Path, session_key: &str) -> Json {
    if !path.is_file() {
        return empty_hook_state(session_key);
    }
    let load = (|| -> Option<Json> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() > RADAR_STATE_MAX_BYTES {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let state = Json::parse(&text).ok()?;
        if state.get("version").and_then(Json::as_i64) != Some(HOOK_STATE_VERSION)
            || state.get("session_key").and_then(Json::as_str) != Some(session_key)
            || state.get("cooldown").and_then(Json::as_object).is_none()
            || state
                .get("reservations")
                .and_then(Json::as_object)
                .is_none()
        {
            return None;
        }
        Some(state)
    })();
    load.unwrap_or_else(|| empty_hook_state(session_key))
}

fn valid_hook_reservation(token: &str, value: &Json, now_ts: f64) -> bool {
    if !(20..=128).contains(&token.len())
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return false;
    }
    let Some(expires) = value.get("expires_at").and_then(Json::as_f64) else {
        return false;
    };
    if !expires.is_finite()
        || !(0.0 < expires - now_ts
            && expires - now_ts <= hook_reservation_ttl().mul_add(2.0, 0.0).max(1.0))
    {
        return false;
    }
    let Some(card_ids) = value.get("card_ids").and_then(Json::as_array) else {
        return false;
    };
    if card_ids.is_empty() || card_ids.len() as i64 > HOOK_MAX_BUDGET {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    for id in card_ids {
        match id.as_i64() {
            Some(value) if value > 0 && seen.insert(value) => {}
            _ => return false,
        }
    }
    true
}

fn prune_hook_state(state: &mut Json, now_ts: f64, ttl: i64) {
    let cooldown = fresh_cooldown(state.get("cooldown"), now_ts, ttl);
    if let Json::Object(ref mut pairs) = *state {
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "cooldown") {
            slot.1 = Json::Object(
                cooldown
                    .into_iter()
                    .map(|(k, v)| (k, Json::Float(v)))
                    .collect(),
            );
        }
        let reservations = pairs
            .iter()
            .find(|(k, _)| k == "reservations")
            .and_then(|(_, v)| v.as_object())
            .map(|pairs| {
                pairs
                    .iter()
                    .filter(|(token, value)| valid_hook_reservation(token, value, now_ts))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "reservations") {
            slot.1 = Json::Object(reservations);
        }
    }
}

fn cap_hook_cooldown(state: &mut Json, cap: usize) {
    if let Some(cooldown) = state
        .get("cooldown")
        .and_then(Json::as_object)
        .map(|pairs| pairs.to_vec())
    {
        if cooldown.len() > cap {
            let mut ordered = cooldown;
            ordered.sort_by(|a, b| {
                b.1.as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&a.1.as_f64().unwrap_or(0.0))
            });
            ordered.truncate(cap);
            if let Json::Object(ref mut pairs) = *state {
                if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "cooldown") {
                    slot.1 = Json::Object(ordered);
                }
            }
        }
    }
}

pub fn hook_fast_scan(layout: &Layout, payload: &Json) -> Result<Json> {
    match hook_fast_scan_inner(layout, payload) {
        Ok(result) => Ok(result),
        Err(err @ Error::HookProtocol(_)) => Err(err),
        Err(Error::HookUnavailable(reason)) => Ok(hook_unavailable(reason)),
        Err(Error::HookDeadlineExceeded) => Ok(hook_unavailable("timeout")),
        Err(_) => Ok(hook_unavailable("internal")),
    }
}

fn hook_fast_scan_inner(layout: &Layout, payload: &Json) -> Result<Json> {
    validate_hook_scan_payload(payload)?;
    let budget = payload.get("budget").and_then(Json::as_i64).unwrap_or(0);
    let text = payload.get("text").and_then(Json::as_str).unwrap_or("");
    if budget == 0 || text.trim().is_empty() {
        return Ok(crate::jobject! {
            "protocol_version" => HOOK_PROTOCOL_VERSION,
            "status" => "ok",
            "items" => Json::Array(Vec::new()),
        });
    }
    let deadline = Instant::now() + Duration::from_secs_f64(hook_fast_timeout().max(0.0));
    let cfg = config::load_config(&layout.home);
    let radar_cfg = config::section(&cfg, "radar");
    let gist_limit = radar_gist_limit(radar_cfg);
    let candidates = hook_cache_candidates(layout, payload, deadline, gist_limit)?;
    if candidates.is_empty() {
        return Ok(crate::jobject! {
            "protocol_version" => HOOK_PROTOCOL_VERSION,
            "status" => "ok",
            "items" => Json::Array(Vec::new()),
        });
    }
    let host = payload.get("host").and_then(Json::as_str).unwrap_or("");
    let session_id = payload
        .get("session_id")
        .and_then(Json::as_str)
        .unwrap_or("");
    let session_key = hook_session_key(host, session_id);
    let state_path = hook_state_path(layout, &session_key);
    let ttl_value = ttl(radar_cfg);
    let lock_timeout = deadline_remaining(deadline)?.as_secs_f64().min(0.1);
    let token;
    let selected;
    {
        let _state =
            FileLock::acquire(layout, "radar-state", false, Some(lock_timeout)).map_err(|err| {
                match err {
                    Error::LockTimeout(_) => Error::HookUnavailable("cache_busy"),
                    other => other,
                }
            })?;
        let now_ts = clock().unix_seconds();
        let mut state = load_hook_state(&state_path, &session_key);
        prune_hook_state(&mut state, now_ts, ttl_value);
        let reservations = state
            .get("reservations")
            .and_then(Json::as_object)
            .map(|pairs| pairs.to_vec())
            .unwrap_or_default();
        if reservations.len() >= 128 {
            return Err(Error::HookUnavailable("internal"));
        }
        let mut blocked: std::collections::HashSet<String> = state
            .get("cooldown")
            .and_then(Json::as_object)
            .map(|pairs| pairs.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        for (_, reservation) in &reservations {
            if let Some(card_ids) = reservation.get("card_ids").and_then(Json::as_array) {
                for id in card_ids {
                    if let Some(value) = id.as_i64() {
                        blocked.insert(value.to_string());
                    }
                }
            }
        }
        let available: Vec<Json> = candidates
            .into_iter()
            .filter(|candidate| {
                let id = candidate.get("id").and_then(Json::as_i64).unwrap_or(0);
                !blocked.contains(&id.to_string())
            })
            .collect();
        selected = pack_radar_candidates(&available, budget, HOOK_BLOCK_PREFIX, HOOK_BLOCK_SUFFIX);
        if selected.is_empty() {
            return Ok(crate::jobject! {
                "protocol_version" => HOOK_PROTOCOL_VERSION,
                "status" => "ok",
                "items" => Json::Array(Vec::new()),
            });
        }
        token = clock().urlsafe_token();
        let reservation = crate::jobject! {
            "card_ids" => Json::Array(selected.iter().filter_map(|item| item.get("id").cloned()).collect()),
            "expires_at" => now_ts + hook_reservation_ttl(),
        };
        if let Json::Object(ref mut pairs) = state {
            if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "reservations") {
                if let Json::Object(ref mut reservations) = slot.1 {
                    reservations.push((token.clone(), reservation));
                }
            }
        }
        if let Some(parent) = state_path.parent() {
            let _ = crate::durable_fs::create_dir_all_private(parent);
        }
        atomic_write(&state_path, &state.dumps_canonical())
            .map_err(|err| Error::core(err.to_string()))?;
    }
    Ok(crate::jobject! {
        "protocol_version" => HOOK_PROTOCOL_VERSION,
        "status" => "ok",
        "items" => Json::Array(selected.iter().map(|item| crate::jobject! {
            "id" => item.get("id").and_then(Json::as_i64).unwrap_or(0),
            "line" => item.get("line").and_then(Json::as_str).unwrap_or("").to_string(),
        }).collect()),
        "reservation_id" => token,
        "session_key" => session_key,
    })
}

pub fn hook_control(layout: &Layout, payload: &Json, commit: bool) -> Result<Json> {
    match hook_control_inner(layout, payload, commit) {
        Ok(value) => Ok(value),
        Err(err @ Error::HookProtocol(_)) => Err(err),
        Err(Error::LockTimeout(_)) => Ok(crate::jobject! {
            "protocol_version" => HOOK_PROTOCOL_VERSION,
            "status" => "unavailable",
            "applied" => false,
            "reason" => "cache_busy",
        }),
        Err(_) => Ok(crate::jobject! {
            "protocol_version" => HOOK_PROTOCOL_VERSION,
            "status" => "unavailable",
            "applied" => false,
            "reason" => "internal",
        }),
    }
}

fn hook_control_inner(layout: &Layout, payload: &Json, commit: bool) -> Result<Json> {
    validate_hook_control_payload(payload)?;
    let session_key = payload
        .get("session_key")
        .and_then(Json::as_str)
        .unwrap_or("");
    let path = hook_state_path(layout, session_key);
    if !path.is_file() || !layout.locks().is_dir() {
        return Ok(crate::jobject! {
            "protocol_version" => HOOK_PROTOCOL_VERSION,
            "status" => "ok",
            "applied" => false,
        });
    }
    let cfg = config::load_config(&layout.home);
    let radar_cfg = config::section(&cfg, "radar");
    let ttl_value = ttl(radar_cfg);
    let cap = config::get(radar_cfg, "cooldown_max_entries")
        .and_then(config::py_int)
        .unwrap_or(1024)
        .max(16) as usize;
    let applied;
    {
        let _state = FileLock::acquire(layout, "radar-state", false, Some(0.2))?;
        let now_ts = clock().unix_seconds();
        let mut state = load_hook_state(&path, session_key);
        prune_hook_state(&mut state, now_ts, ttl_value);
        let reservation_id = payload
            .get("reservation_id")
            .and_then(Json::as_str)
            .unwrap_or("");
        let mut reservation = None;
        if let Json::Object(ref mut pairs) = state {
            if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "reservations") {
                if let Json::Object(ref mut reservations) = slot.1 {
                    if let Some(position) =
                        reservations.iter().position(|(k, _)| k == reservation_id)
                    {
                        reservation = Some(reservations.remove(position).1);
                    }
                }
            }
        }
        applied = reservation.is_some();
        if let Some(reservation) = reservation {
            if commit {
                if let Some(card_ids) = reservation.get("card_ids").and_then(Json::as_array) {
                    if let Json::Object(ref mut pairs) = state {
                        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "cooldown") {
                            if let Json::Object(ref mut cooldown) = slot.1 {
                                for id in card_ids {
                                    if let Some(value) = id.as_i64() {
                                        let key = value.to_string();
                                        cooldown.retain(|(k, _)| k != &key);
                                        cooldown.push((key, Json::Float(now_ts)));
                                    }
                                }
                            }
                        }
                    }
                }
                cap_hook_cooldown(&mut state, cap);
            }
        }
        atomic_write(&path, &state.dumps_canonical())
            .map_err(|err| Error::core(err.to_string()))?;
    }
    Ok(crate::jobject! {
        "protocol_version" => HOOK_PROTOCOL_VERSION,
        "status" => "ok",
        "applied" => applied,
    })
}

// --- Codex hook entry points (fail-open; never disturb the host) ---

fn codex_read_event() -> Json {
    let mut input = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err() {
        return Json::Object(Vec::new());
    }
    Json::parse(&input).unwrap_or(Json::Object(Vec::new()))
}

fn codex_extract_text(event: &Json, keys: &[&str]) -> String {
    for key in keys {
        let Some(value) = event.get(key) else {
            continue;
        };
        if let Some(text) = value.as_str() {
            if !text.trim().is_empty() {
                return text.to_string();
            }
        } else if value.is_object() {
            for sub in ["text", "content", "message"] {
                if let Some(text) = value.get(sub).and_then(Json::as_str) {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

fn codex_emit_context(event_name: &str, text: &str) {
    let payload = crate::jobject! {
        "hookSpecificOutput" => crate::jobject! {
            "hookEventName" => event_name,
            "additionalContext" => text,
        },
    };
    println!("{}", payload.dumps());
}

fn codex_project_of(event: &Json, layout: &Layout) -> String {
    let cwd = event.get("cwd").and_then(Json::as_str).unwrap_or("");
    if cwd.is_empty() {
        return "global".into();
    }
    let project = project_context_id(Some(cwd), true, layout);
    if project.is_empty() {
        "global".into()
    } else {
        project
    }
}

fn codex_valid_context(value: &str) -> bool {
    if !value.starts_with(CODEX_BLOCK_PREFIX) || value.len() > HOOK_MAX_BLOCK_BYTES {
        return false;
    }
    let body = &value[CODEX_BLOCK_PREFIX.len()..];
    let lines: Vec<&str> = body.split('\n').collect();
    if lines.is_empty() || lines.len() > 3 {
        return false;
    }
    let mut memory_ids = std::collections::HashSet::new();
    for line in lines {
        let Some(rest) = line.strip_prefix("记忆提示：记忆 ") else {
            return false;
        };
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if digits.is_empty() || digits.starts_with('0') {
            return false;
        }
        let Some(after) = rest[digits.len()..].strip_prefix("：") else {
            return false;
        };
        let Ok(memory_id) = digits.parse::<i64>() else {
            return false;
        };
        if !memory_ids.insert(memory_id) {
            return false;
        }
        if after.is_empty()
            || py_len(line) > HOOK_MAX_LINE_CODEPOINTS
            || line.len() > HOOK_MAX_LINE_BYTES
            || hook_control_character(line)
            || line.contains("[long-term-memory-index:")
            || line.contains("[/long-term-memory-index]")
        {
            return false;
        }
    }
    true
}

pub fn codex_user_prompt_submit(layout: &Layout) {
    let event = codex_read_event();
    let prompt = codex_extract_text(
        &event,
        &["prompt", "user_prompt", "input", "message", "text"],
    );
    let session = event
        .get("session_id")
        .and_then(Json::as_str)
        .unwrap_or("unknown")
        .to_string();
    let project = codex_project_of(&event, layout);
    if prompt.trim().is_empty() {
        return;
    }
    if let Ok(result) = scan_text(layout, &prompt, &session, None, &project) {
        if let Some(context) = result.get("context").and_then(Json::as_str) {
            if !context.is_empty() && codex_valid_context(context) {
                codex_emit_context("UserPromptSubmit", context);
            }
        }
    }
}

pub fn codex_session_start(layout: &Layout) {
    let event = codex_read_event();
    let source = event
        .get("source")
        .and_then(Json::as_str)
        .unwrap_or("startup");
    if source != "resume" && source != "compact" {
        return;
    }
    let project = codex_project_of(&event, layout);
    let cfg = config::load_config(&layout.home);
    if let Ok(rows) = crate::search::search(layout, "", "published", 2, &project, Some(&cfg)) {
        let lines: Vec<String> = rows.iter().map(crate::textops::human_index_line).collect();
        if !lines.is_empty() {
            codex_emit_context(
                "SessionStart",
                &format!(
                    "Engramark 长期记忆（高强度，需要详情可 memory_get）：\n{}",
                    lines.join("\n")
                ),
            );
        }
    }
}

/// Legacy no-op events (PostToolUse/Stop/SessionEnd): exit 0 with no output.
pub fn codex_noop() {}
