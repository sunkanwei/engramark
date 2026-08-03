//! Search: five recall lanes, RRF k=60 fusion, evidence scoring, thresholds
//! and stable tie-breaking. Floating point is IEEE-754 double in the frozen
//! operation order; no fast-math, no FMA contraction, no unordered maps in
//! the ranking path.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::anchors::{anchor_present, char_grams, Anchor};
use crate::cache::{self, row_to_meta, QUERY_TIMEOUT_MS_DEFAULT};
use crate::config;
use crate::difflib;
use crate::json::Json;
use crate::normalize::normalize_text;
use crate::paths::Layout;
use crate::query::{plan_query, QueryPlan, QueryTerm, CONTENT_INTENT_TERMS};
use crate::radar::scope_visible;
use crate::textops::MetaRow;
use crate::{Error, Result, MAX_QUERY_CHARS};

fn fts_quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn status_filter(scope: &str) -> &'static str {
    match scope {
        "candidate" => "m.status='candidate'",
        "all" => "1=1",
        _ => "m.status='published'",
    }
}

fn scope_filter(project: &str) -> (String, Vec<String>) {
    let global_clause = "(COALESCE(m.scope, '') = '' OR LOWER(m.scope) = 'global')";
    if project.is_empty() || project == "global" {
        return (global_clause.to_string(), Vec::new());
    }
    (
        format!("({global_clause} OR LOWER(m.scope) IN (LOWER(?), LOWER(?)))"),
        vec![project.to_string(), format!("project:{project}")],
    )
}

fn run_lane(conn: &Connection, sql: &str, params: &[String], pool: usize) -> Vec<i64> {
    let run = (|| -> rusqlite::Result<Vec<i64>> {
        let mut stmt = conn.prepare(sql)?;
        let sql_params: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(sql_params.as_slice(), |row| row.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for row in rows.take(pool) {
            out.push(row?);
        }
        Ok(out)
    })();
    run.unwrap_or_default()
}

fn scope_affinity(card_scope: &str, project: &str) -> f64 {
    let scope = normalize_text(card_scope);
    if scope.is_empty() || scope == "global" {
        return 0.0;
    }
    if scope_visible(card_scope, project) {
        1.0
    } else {
        -1.0
    }
}

fn percent0(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

fn card_evidence(
    row: &MetaRow,
    plan: &QueryPlan,
    card_anchors: &[Anchor],
    lanes: &std::collections::BTreeSet<String>,
    project: &str,
) -> (f64, String, bool) {
    let title = normalize_text(&row.title);
    let body = normalize_text(&row.body);
    let entities = normalize_text(&row.entities);
    let all_text = format!("{title}\n{body}\n{entities}");
    let terms = &plan.terms;
    let mut matched_weight = 0.0f64;
    let total_weight: f64 = {
        let sum: f64 = terms.iter().map(|t| t.weight).sum();
        if sum == 0.0 {
            1.0
        } else {
            sum
        }
    };
    let mut title_weight = 0.0f64;
    let mut entity_weight = 0.0f64;
    let intent_terms: Vec<&QueryTerm> = terms
        .iter()
        .filter(|t| CONTENT_INTENT_TERMS.contains(&t.norm.as_str()))
        .collect();
    let mut intent_focus = 0.0f64;
    for term in terms {
        if all_text.contains(&term.norm) {
            matched_weight += term.weight;
        }
        if title.contains(&term.norm) {
            title_weight += term.weight;
        }
        if entities.contains(&term.norm) {
            entity_weight += term.weight;
        }
        if intent_terms.iter().any(|t| t.norm == term.norm)
            && (title.contains(&term.norm) || entities.contains(&term.norm))
        {
            intent_focus += term.weight;
        }
    }
    let mut fuzzy_weight = 0.0f64;
    let mut fuzzy_strong = false;
    for term in terms {
        if term.generic || term.norm.chars().count() < 4 || all_text.contains(&term.norm) {
            continue;
        }
        let mut best = (0.0f64, false);
        for anchor in card_anchors {
            let ratio = difflib::ratio(&term.norm, &anchor.norm);
            if ratio > best.0 {
                best = (ratio, anchor.strong);
            }
        }
        if best.0 >= 0.78 {
            fuzzy_weight += term.weight * best.0.min(1.0);
            fuzzy_strong = fuzzy_strong || best.1;
        }
    }
    let coverage = ((matched_weight + 0.75 * fuzzy_weight) / total_weight).min(1.0);
    let matched_anchors: Vec<&Anchor> = card_anchors
        .iter()
        .filter(|anchor| anchor_present(&anchor.norm, &plan.norm))
        .collect();
    let strong_anchor = matched_anchors.iter().any(|anchor| anchor.strong);
    let weak_anchor = !matched_anchors.is_empty() && !strong_anchor;
    let exact_phrase = !plan.norm.is_empty() && all_text.contains(&plan.norm);
    let mut score = 0.0f64;
    if strong_anchor {
        score += 0.43;
    } else if weak_anchor {
        score += 0.18;
    }
    if !strong_anchor && fuzzy_strong {
        score += 0.25;
    }
    score += 0.32 * coverage;
    score += 0.10 * ((title_weight + 1.25 * entity_weight) / total_weight).min(1.0);
    if !intent_terms.is_empty() {
        let intent_total: f64 = {
            let sum: f64 = intent_terms.iter().map(|t| t.weight).sum();
            if sum == 0.0 {
                1.0
            } else {
                sum
            }
        };
        score += 0.12 * (intent_focus / intent_total).min(1.0);
    }
    if exact_phrase {
        score += 0.07;
    }
    let affinity = scope_affinity(&row.scope, project);
    if affinity > 0.0 {
        score += 0.06;
    } else if affinity < 0.0 {
        score -= 0.05;
    }
    score += (0.012 * lanes.len() as f64).min(0.04);
    score += 0.03 * ((row.i + row.t) as f64 / 9.0);
    let only_generic = !terms.is_empty() && terms.iter().all(|t| t.generic);
    if only_generic && matched_anchors.is_empty() {
        score -= 0.18;
    }
    let mut signals: Vec<String> = Vec::new();
    if strong_anchor {
        signals.push("强锚点".into());
    } else if weak_anchor {
        signals.push("弱锚点".into());
    } else if fuzzy_strong {
        signals.push("近似强锚点".into());
    }
    if coverage != 0.0 {
        signals.push(format!("词项覆盖 {}", percent0(coverage)));
    }
    if exact_phrase {
        signals.push("原短语".into());
    }
    if intent_focus != 0.0 {
        signals.push("标题/实体意图".into());
    }
    if affinity > 0.0 {
        signals.push("项目作用域".into());
    }
    (
        score.clamp(0.0, 1.0),
        if signals.is_empty() {
            "低证据".into()
        } else {
            signals.join("、")
        },
        strong_anchor,
    )
}

fn like_scores(
    conn: &Connection,
    terms: &[String],
    clause: &str,
    scope_params: &[String],
) -> Vec<i64> {
    // Insertion-ordered counts (Python dict order: first-seen wins ties).
    let mut counts: Vec<(i64, i64)> = Vec::new();
    for term in terms {
        let pattern = format!("%{term}%");
        let sql = format!(
            "SELECT m.id FROM meta m WHERE {clause} AND \
             (m.title LIKE ? OR m.body LIKE ? OR m.entities LIKE ? OR EXISTS(\
             SELECT 1 FROM anchors a WHERE a.card_id=m.id AND a.norm LIKE ?))"
        );
        let mut params: Vec<String> = scope_params.to_vec();
        params.extend([pattern.clone(), pattern.clone(), pattern.clone(), pattern]);
        let run = (|| -> rusqlite::Result<Vec<i64>> {
            let mut stmt = conn.prepare(&sql)?;
            let sql_params: Vec<&dyn rusqlite::ToSql> =
                params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(sql_params.as_slice(), |row| row.get::<_, i64>(0))?;
            rows.collect()
        })();
        for cid in run.unwrap_or_default() {
            match counts.iter_mut().find(|(id, _)| *id == cid) {
                Some((_, count)) => *count += 1,
                None => counts.push((cid, 1)),
            }
        }
    }
    // Python: sorted(counts.items(), key=count, reverse=True) — stable.
    let mut ordered = counts;
    ordered.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    ordered.into_iter().map(|(cid, _)| cid).collect()
}

pub fn search(
    layout: &Layout,
    query: &str,
    scope: &str,
    limit: i64,
    project: &str,
    cfg: Option<&Json>,
) -> Result<Vec<MetaRow>> {
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(Error::core(format!("查询超过 {MAX_QUERY_CHARS} 字符上限")));
    }
    let limit = limit.clamp(1, 20) as usize;
    let (scope_clause, scope_params) = scope_filter(project);
    let clause = format!("{} AND {scope_clause}", status_filter(scope));
    let default_cfg;
    let cfg = match cfg {
        Some(cfg) => cfg,
        None => {
            default_cfg = config::load_config(&layout.home);
            &default_cfg
        }
    };
    let reader = cache::cache_reader(layout)?;
    let conn = &reader.conn;
    let timeout_ms = config::get(config::section(cfg, "search"), "query_timeout_ms")
        .and_then(config::py_int)
        .unwrap_or(QUERY_TIMEOUT_MS_DEFAULT)
        .clamp(25, 5000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
    cache::set_query_deadline(conn, Some(deadline));
    let result = search_inner(
        conn,
        layout,
        query,
        scope,
        limit,
        project,
        cfg,
        &clause,
        &scope_clause,
        &scope_params,
    );
    cache::set_query_deadline(conn, None);
    match result {
        Err(err) if is_timeout_error(&err) => Err(Error::core("查询超过时间预算，已安全中止")),
        other => other,
    }
}

fn is_timeout_error(err: &Error) -> bool {
    match err {
        Error::CacheUnavailable(message) => message.contains("interrupted"),
        _ => false,
    }
}

fn map_sql(err: rusqlite::Error) -> Error {
    if cache::is_interrupt(&err) {
        return Error::CacheUnavailable("interrupted".into());
    }
    Error::cache(format!("缓存读取失败：{err}"))
}

#[allow(clippy::too_many_arguments)]
fn search_inner(
    conn: &Connection,
    _layout: &Layout,
    query: &str,
    _scope: &str,
    limit: usize,
    project: &str,
    cfg: &Json,
    clause: &str,
    scope_clause: &str,
    scope_params: &[String],
) -> Result<Vec<MetaRow>> {
    let mut superseded = std::collections::HashSet::new();
    {
        let sql = format!(
            "SELECT m.id,m.supersedes,m.valid_from,m.valid_to FROM meta m \
             WHERE m.status='published' AND m.supersedes<>'' AND {scope_clause}"
        );
        let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
        let sql_params: Vec<&dyn rusqlite::ToSql> = scope_params
            .iter()
            .map(|p| p as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(sql_params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(map_sql)?;
        for row in rows {
            let (supersedes, valid_from, valid_to) = row.map_err(map_sql)?;
            let meta = MetaRow {
                valid_from,
                valid_to,
                ..MetaRow::default()
            };
            if meta.is_current() {
                for cid in supersedes.split(',') {
                    if !cid.is_empty() && cid.bytes().all(|b| b.is_ascii_digit()) {
                        if let Ok(value) = cid.parse::<i64>() {
                            superseded.insert(value);
                        }
                    }
                }
            }
        }
    }
    if query.trim().is_empty() {
        let sql = format!("SELECT m.* FROM meta m WHERE {clause}");
        let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
        let sql_params: Vec<&dyn rusqlite::ToSql> = scope_params
            .iter()
            .map(|p| p as &dyn rusqlite::ToSql)
            .collect();
        let mapped = stmt
            .query_map(sql_params.as_slice(), row_to_meta)
            .map_err(map_sql)?;
        let mut rows: Vec<MetaRow> = Vec::new();
        for row in mapped {
            let row: MetaRow = row.map_err(map_sql)?;
            if row.is_current() && !superseded.contains(&row.id) {
                rows.push(row);
            }
        }
        rows.sort_by(|a, b| {
            let ka = a.rank_key();
            let kb = b.rank_key();
            kb.0.cmp(&ka.0)
                .then(kb.1.cmp(&ka.1))
                .then(kb.2.cmp(&ka.2))
                .then(kb.3.total_cmp(&ka.3))
        });
        rows.truncate(limit);
        return Ok(rows);
    }
    let plan = plan_query(query, cfg);
    if plan.terms.is_empty() {
        return Ok(Vec::new());
    }
    let search_cfg = config::section(cfg, "search");
    let pool = (config::get(search_cfg, "candidate_pool")
        .and_then(config::py_int)
        .unwrap_or(80))
    .max(limit as i64 * 8) as usize;
    let mut lanes: Vec<(&'static str, Vec<i64>)> = Vec::new();
    let specific: Vec<&QueryTerm> = plan.terms.iter().filter(|t| !t.generic).collect();
    let focused_expr = if specific.is_empty() {
        plan.terms
            .iter()
            .map(|t| fts_quote(&t.norm))
            .collect::<Vec<_>>()
            .join(" OR ")
    } else {
        specific
            .iter()
            .map(|t| fts_quote(&t.norm))
            .collect::<Vec<_>>()
            .join(" OR ")
    };
    {
        let mut params: Vec<String> = scope_params.to_vec();
        params.push(focused_expr);
        let sql = format!(
            "SELECT f.rowid FROM fts f JOIN meta m ON m.id=f.rowid \
             WHERE {clause} AND fts MATCH ? ORDER BY bm25(fts,3.0,1.0,5.0,4.0)"
        );
        lanes.push(("terms", run_lane(conn, &sql, &params, pool)));
        let mut params: Vec<String> = scope_params.to_vec();
        params.push(fts_quote(&plan.norm));
        lanes.push(("phrase", run_lane(conn, &sql, &params, pool)));
        let fuzzy_terms: Vec<String> = plan
            .terms
            .iter()
            .filter(|t| t.norm.chars().count() >= 3)
            .map(|t| t.norm.clone())
            .collect();
        if !fuzzy_terms.is_empty() {
            let mut params: Vec<String> = scope_params.to_vec();
            params.push(
                fuzzy_terms
                    .iter()
                    .map(|t| fts_quote(t))
                    .collect::<Vec<_>>()
                    .join(" OR "),
            );
            let sql = format!(
                "SELECT f.rowid FROM fts_tri f JOIN meta m ON m.id=f.rowid \
                 WHERE {clause} AND fts_tri MATCH ? ORDER BY bm25(fts_tri,3.0,1.0,5.0,4.0)"
            );
            lanes.push(("trigram", run_lane(conn, &sql, &params, pool)));
        }
    }
    let term_norms: Vec<String> = plan.terms.iter().map(|t| t.norm.clone()).collect();
    {
        let marks = term_norms.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT a.card_id,a.norm,a.strength,a.manual FROM anchors a JOIN meta m ON m.id=a.card_id \
             WHERE {clause} AND a.norm IN ({marks})"
        );
        let mut params: Vec<String> = scope_params.to_vec();
        params.extend(term_norms.iter().cloned());
        let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
        let sql_params: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(sql_params.as_slice(), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(map_sql)?;
        let mut anchor_rows: Vec<(i64, String, String, i64)> = Vec::new();
        for row in rows {
            anchor_rows.push(row.map_err(map_sql)?);
        }
        anchor_rows.sort_by(|a, b| {
            let ka = (a.2 == "strong", a.3 == 1, a.1.chars().count());
            let kb = (b.2 == "strong", b.3 == 1, b.1.chars().count());
            kb.cmp(&ka)
        });
        let mut seen = std::collections::HashSet::new();
        let anchors_lane: Vec<i64> = anchor_rows
            .iter()
            .filter_map(|(cid, _, _, _)| if seen.insert(*cid) { Some(*cid) } else { None })
            .take(pool)
            .collect();
        lanes.push(("anchors", anchors_lane));
    }
    let mut fuzzy_grams = std::collections::BTreeSet::new();
    for term in plan
        .terms
        .iter()
        .filter(|t| !t.generic && t.norm.chars().count() >= 4)
    {
        fuzzy_grams.extend(char_grams(&term.norm));
    }
    if !fuzzy_grams.is_empty() {
        let grams: Vec<String> = fuzzy_grams.into_iter().collect();
        let gram_marks = grams.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT g.card_id,COUNT(DISTINCT g.gram) AS overlap FROM anchor_grams g \
             JOIN meta m ON m.id=g.card_id WHERE {clause} AND g.gram IN ({gram_marks}) \
             GROUP BY g.card_id ORDER BY overlap DESC"
        );
        let mut params: Vec<String> = scope_params.to_vec();
        params.extend(grams);
        lanes.push(("fuzzy_anchor", run_lane(conn, &sql, &params, pool)));
    }
    let substring = like_scores(conn, &term_norms, clause, scope_params);
    lanes.push(("substring", substring.into_iter().take(pool).collect()));

    // Insertion-ordered RRF map (Python defaultdict insertion order decides
    // tie order after the stable descending sort).
    let mut rrf: Vec<(i64, f64)> = Vec::new();
    let mut card_lanes: BTreeMap<i64, std::collections::BTreeSet<String>> = BTreeMap::new();
    let k = config::get(search_cfg, "rrf_k")
        .and_then(config::py_int)
        .unwrap_or(60);
    for (lane, ids) in &lanes {
        for (rank, cid) in ids.iter().enumerate() {
            match rrf.iter_mut().find(|(id, _)| id == cid) {
                Some((_, score)) => *score += 1.0 / (k + 1 + rank as i64) as f64,
                None => rrf.push((*cid, 1.0 / (k + 1 + rank as i64) as f64)),
            }
            card_lanes
                .entry(*cid)
                .or_default()
                .insert((*lane).to_string());
        }
    }
    if rrf.is_empty() {
        return Ok(Vec::new());
    }
    // Python: sorted(rrf, key=rrf.get, reverse=True) — stable.
    let mut rrf_sorted = rrf.clone();
    rrf_sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    let rrf_of = |cid: i64| {
        rrf.iter()
            .find(|(id, _)| *id == cid)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    };
    let mut candidate_ids: Vec<i64> = rrf_sorted.iter().map(|(cid, _)| *cid).collect();
    candidate_ids.truncate(pool);
    let marks = candidate_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let mut detail_rows: Vec<MetaRow> = Vec::new();
    {
        let sql = format!(
            "SELECT m.*,m.title AS _title,m.body AS _body,m.entities AS _entities \
             FROM meta m WHERE {clause} AND m.id IN ({marks})"
        );
        let int_params: Vec<i64> = candidate_ids.clone();
        let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
        let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for p in scope_params {
            sql_params.push(p);
        }
        for p in &int_params {
            sql_params.push(p);
        }
        let rows = stmt
            .query_map(sql_params.as_slice(), row_to_meta)
            .map_err(map_sql)?;
        for row in rows {
            detail_rows.push(row.map_err(map_sql)?);
        }
    }
    let mut anchors_by_card: BTreeMap<i64, Vec<Anchor>> = BTreeMap::new();
    {
        let sql =
            format!("SELECT card_id,norm,strength,manual FROM anchors WHERE card_id IN ({marks})");
        let mut stmt = conn.prepare(&sql).map_err(map_sql)?;
        let mut sql_params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for cid in &candidate_ids {
            sql_params.push(cid);
        }
        let rows = stmt
            .query_map(sql_params.as_slice(), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(map_sql)?;
        for row in rows {
            let (card_id, norm, strength, manual) = row.map_err(map_sql)?;
            anchors_by_card.entry(card_id).or_default().push(Anchor {
                value: String::new(),
                norm,
                kind: String::new(),
                strong: strength == "strong",
                manual: manual == 1,
            });
        }
    }
    let mut scored: Vec<MetaRow> = Vec::new();
    for mut row in detail_rows {
        if !row.is_current() || superseded.contains(&row.id) {
            continue;
        }
        let empty = Vec::new();
        let anchors = anchors_by_card.get(&row.id).unwrap_or(&empty);
        let lanes_of_card = card_lanes.get(&row.id).cloned().unwrap_or_default();
        let (score, evidence, strong_anchor) =
            card_evidence(&row, &plan, anchors, &lanes_of_card, project);
        row.score = score;
        row.evidence = evidence;
        row.strong_anchor = strong_anchor;
        row.rrf = rrf_of(row.id);
        scored.push(row);
    }
    // Stable sort on (_score, _rrf, lock, i, t, freshness) descending.
    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(b.rrf.total_cmp(&a.rrf))
            .then_with(|| {
                let ka = a.rank_key();
                let kb = b.rank_key();
                kb.0.cmp(&ka.0)
                    .then(kb.1.cmp(&ka.1))
                    .then(kb.2.cmp(&ka.2))
                    .then(kb.3.total_cmp(&ka.3))
            })
    });
    let high = config::get(search_cfg, "high_threshold")
        .and_then(config::py_float)
        .unwrap_or(0.64);
    let medium = config::get(search_cfg, "medium_threshold")
        .and_then(config::py_float)
        .unwrap_or(0.34);
    let second_score = scored.get(1).map(|row| row.score);
    let scored_len = scored.len();
    let mut accepted: Vec<MetaRow> = Vec::new();
    for (position, mut row) in scored.into_iter().enumerate() {
        if row.score < medium {
            continue;
        }
        let mut confidence = if row.score >= high { "high" } else { "medium" };
        if position == 0 && confidence == "high" && scored_len > 1 {
            let margin = row.score - second_score.unwrap_or(0.0);
            if margin < 0.06 && !row.strong_anchor {
                confidence = "medium";
            }
        }
        row.confidence = confidence.to_string();
        accepted.push(row);
    }
    accepted.truncate(limit);
    Ok(accepted)
}
