//! Immutable compatibility fixtures captured when the native migration was
//! completed. Contract changes must explicitly update the affected fixtures,
//! their checksum manifest, the implementation and user-facing documentation.

use std::path::{Path, PathBuf};

use engramark::json::Json;
use engramark::paths::Layout;

const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/golden");
const FAKE_NOW: &str = "2026-08-02T12:00:00";

#[test]
fn golden_manifest_integrity() {
    let directory = Path::new(GOLDEN_DIR);
    let manifest_text =
        std::fs::read_to_string(directory.join("manifest.json")).expect("read golden manifest");
    let manifest = Json::parse(&manifest_text).expect("parse golden manifest");
    assert_eq!(manifest.get("format").and_then(Json::as_i64), Some(1));
    let source_commit = manifest
        .get("source_commit")
        .and_then(Json::as_str)
        .expect("source_commit");
    assert!(
        source_commit.len() == 40 && source_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid source_commit"
    );
    let checksums = manifest
        .get("sha256")
        .and_then(Json::as_object)
        .expect("sha256 object");
    let mut declared = Vec::new();
    for (name, expected) in checksums {
        assert!(
            name.ends_with(".json") && !name.contains('/') && name != "manifest.json",
            "invalid golden path: {name}"
        );
        let bytes = std::fs::read(directory.join(name))
            .unwrap_or_else(|err| panic!("read golden {name}: {err}"));
        assert_eq!(
            engramark::hash::sha256_hex(&bytes),
            expected.as_str().expect("checksum string"),
            "checksum mismatch: {name}"
        );
        declared.push(name.clone());
    }
    declared.sort();
    let mut actual: Vec<String> = std::fs::read_dir(directory)
        .expect("read golden directory")
        .map(|entry| entry.expect("golden directory entry").file_name())
        .filter_map(|name| name.into_string().ok())
        .filter(|name| name.ends_with(".json") && name != "manifest.json")
        .collect();
    actual.sort();
    assert_eq!(actual, declared, "golden file set differs from manifest");
}

fn load(name: &str) -> Json {
    std::env::set_var("ENGRAMARK_TEST_NOW", FAKE_NOW);
    let path = Path::new(GOLDEN_DIR).join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    let doc = Json::parse(&text).expect("fixture parses");
    doc.get("cases").cloned().unwrap_or(Json::Null)
}

fn default_config() -> Json {
    engramark::config::default_config()
}

#[test]
fn normalize_golden() {
    let cases = load("normalize.json");
    for case in cases.as_array().expect("array") {
        let input = case.get("input").and_then(Json::as_str).expect("input");
        let expected = case
            .get("normalized")
            .and_then(Json::as_str)
            .expect("normalized");
        assert_eq!(
            engramark::normalize::normalize_text(input),
            expected,
            "normalize_text({input:?})"
        );
        let expected_cjk = case
            .get("contains_cjk")
            .and_then(Json::as_bool)
            .expect("contains_cjk");
        assert_eq!(
            engramark::normalize::contains_cjk(input),
            expected_cjk,
            "contains_cjk({input:?})"
        );
    }
}

#[test]
fn mem_golden() {
    let cases = load("mem.json");
    for case in cases.as_array().expect("array") {
        let name = case.get("name").and_then(Json::as_str).expect("name");
        let input = case.get("input").and_then(Json::as_str).expect("input");
        let status = case.get("status").and_then(Json::as_str).expect("status");
        match status {
            "ok" => {
                let card = engramark::mem::parse_card(input)
                    .unwrap_or_else(|err| panic!("{name}: parse failed: {err}"));
                let canonical = case
                    .get("canonical")
                    .and_then(Json::as_str)
                    .expect("canonical");
                assert_eq!(
                    engramark::mem::serialize_card(&card),
                    canonical,
                    "{name}: canonical"
                );
                let semantic = case
                    .get("semantic_hash")
                    .and_then(Json::as_str)
                    .expect("semantic_hash");
                assert_eq!(
                    engramark::hash::semantic_hash(&card),
                    semantic,
                    "{name}: hash"
                );
                let entities: Vec<String> = case
                    .get("entities")
                    .and_then(Json::as_array)
                    .expect("entities")
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect();
                assert_eq!(card.entities, entities, "{name}: entities");
                let supersedes: Vec<i64> = case
                    .get("supersedes")
                    .and_then(Json::as_array)
                    .expect("supersedes")
                    .iter()
                    .filter_map(Json::as_i64)
                    .collect();
                assert_eq!(card.supersedes, supersedes, "{name}: supersedes");
                assert_eq!(
                    card.trust,
                    case.get("trust").and_then(Json::as_i64).expect("trust"),
                    "{name}: trust"
                );
                assert_eq!(
                    card.lock,
                    case.get("lock").and_then(Json::as_bool).expect("lock"),
                    "{name}: lock"
                );
                assert_eq!(
                    card.scope,
                    case.get("scope").and_then(Json::as_str).expect("scope"),
                    "{name}: scope"
                );
            }
            "error" => {
                let expected = case.get("error").and_then(Json::as_str).expect("error");
                match engramark::mem::parse_card(input) {
                    Err(err) => assert_eq!(err.to_string(), expected, "{name}: error text"),
                    Ok(_) => panic!("{name}: expected error {expected:?}, parsed successfully"),
                }
            }
            other => panic!("{name}: unexpected status {other}"),
        }
    }
}

#[test]
fn freshness_golden() {
    let rows = load("freshness.json");
    let mut count = 0usize;
    for row in rows.as_array().expect("array") {
        let days = row.get("days").and_then(Json::as_i64).expect("days");
        let text = row.get("text").and_then(Json::as_str).expect("text");
        assert_eq!(
            engramark::freshness_table::FRESHNESS_TEXT[days as usize],
            text,
            "days={days}"
        );
        count += 1;
    }
    assert_eq!(count, engramark::freshness_table::FRESHNESS_TEXT.len());
}

#[test]
fn hash_golden() {
    let doc = load("hash.json");
    for item in doc
        .get("semantic")
        .and_then(Json::as_array)
        .expect("semantic")
    {
        let mem = item.get("mem").and_then(Json::as_str).expect("mem");
        let card = engramark::mem::parse_card(mem).expect("parse");
        assert_eq!(
            engramark::hash::semantic_hash(&card),
            item.get("semantic_hash")
                .and_then(Json::as_str)
                .expect("hash")
        );
    }
    assert_eq!(
        engramark::hash::source_collection_hash_items(&[
            ("cards/0001.mem".into(), "@1 fact published I3 T3 2026-08-01\n= Alpha, beta\n~ user\n标题一\n正文。\n".as_bytes().to_vec()),
            ("cards/0002.mem".into(), "@2 decision candidate I2 T1.5 2026-07-31\n~ self:test\n# lock\n# scope global\n# supersedes @1\n标题二\n".as_bytes().to_vec()),
            ("cards/0003.mem".into(), vec![0xffu8, 0xfe].into_iter().chain(b" invalid bytes still hashed".iter().copied()).collect()),
        ]),
        doc.get("source_collection")
            .and_then(Json::as_str)
            .expect("source_collection")
    );
    let payload = doc.get("journal_payload").expect("journal_payload");
    assert_eq!(
        engramark::hash::journal_checksum(payload),
        doc.get("journal_checksum")
            .and_then(Json::as_str)
            .expect("journal_checksum")
    );
}

#[test]
fn anchors_golden() {
    let doc = load("anchors.json");
    let cfg = default_config();
    for case in doc.get("cards").and_then(Json::as_array).expect("cards") {
        let name = case.get("name").and_then(Json::as_str).expect("name");
        let mem = case.get("mem").and_then(Json::as_str).expect("mem");
        let card = engramark::mem::parse_card(mem).expect("parse");
        let anchors = engramark::anchors::derive_anchors(&card, &cfg);
        let expected = case
            .get("anchors")
            .and_then(Json::as_array)
            .expect("anchors");
        assert_eq!(anchors.len(), expected.len(), "{name}: anchor count");
        for (actual, expected) in anchors.iter().zip(expected) {
            assert_eq!(
                actual.value,
                expected.get("value").and_then(Json::as_str).unwrap(),
                "{name}"
            );
            assert_eq!(
                actual.norm,
                expected.get("norm").and_then(Json::as_str).unwrap(),
                "{name}"
            );
            assert_eq!(
                actual.kind,
                expected.get("kind").and_then(Json::as_str).unwrap(),
                "{name}"
            );
            assert_eq!(
                actual.strength(),
                expected.get("strength").and_then(Json::as_str).unwrap(),
                "{name}"
            );
            assert_eq!(
                actual.manual,
                expected.get("manual").and_then(Json::as_bool).unwrap(),
                "{name}"
            );
        }
        let trigram = case
            .get("trigram_text")
            .and_then(Json::as_str)
            .expect("trigram_text");
        assert_eq!(trigram_text_of(&card, &anchors), trigram, "{name}: trigram");
    }
    for ratio in doc.get("ratios").and_then(Json::as_array).expect("ratios") {
        let a = ratio.get("a").and_then(Json::as_str).unwrap();
        let b = ratio.get("b").and_then(Json::as_str).unwrap();
        let expected = ratio.get("ratio").and_then(Json::as_f64).unwrap();
        let actual = engramark::difflib::ratio(
            &engramark::normalize::normalize_text(a),
            &engramark::normalize::normalize_text(b),
        );
        assert_eq!(actual, expected, "ratio({a}, {b})");
    }
}

fn trigram_text_of(card: &engramark::mem::Card, anchors: &[engramark::anchors::Anchor]) -> String {
    // Mirrors cache::trigram_text (private) via the public helpers.
    let text = std::iter::once(card.title.as_str())
        .chain(card.body.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    let urls = engramark::pyregex::find_urls(&text);
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
    value
}

#[test]
fn query_plan_golden() {
    let cases = load("query_plan.json");
    let cfg = default_config();
    for case in cases.as_array().expect("array") {
        let query = case.get("query").and_then(Json::as_str).expect("query");
        let plan = engramark::query::plan_query(query, &cfg);
        assert_eq!(
            plan.norm,
            case.get("norm").and_then(Json::as_str).expect("norm"),
            "norm({query:?})"
        );
        let expected = case.get("terms").and_then(Json::as_array).expect("terms");
        assert_eq!(plan.terms.len(), expected.len(), "terms({query:?})");
        for (actual, expected) in plan.terms.iter().zip(expected) {
            assert_eq!(
                actual.text,
                expected.get("text").and_then(Json::as_str).unwrap(),
                "{query:?}"
            );
            assert_eq!(
                actual.norm,
                expected.get("norm").and_then(Json::as_str).unwrap(),
                "{query:?}"
            );
            assert_eq!(
                actual.weight,
                expected.get("weight").and_then(Json::as_f64).unwrap(),
                "{query:?}"
            );
            assert_eq!(
                actual.generic,
                expected.get("generic").and_then(Json::as_bool).unwrap(),
                "{query:?}"
            );
            assert_eq!(
                actual.strong,
                expected.get("strong").and_then(Json::as_bool).unwrap(),
                "{query:?}"
            );
        }
    }
}

const CORPUS: &[(&str, &str)] = &[
    ("0001.mem", "@1 fact published I3 T3 2026-08-01\n= OrchidUI, core\n~ user\n# lock\nOrchidUI（口头称 core）= ~/Library/.../user_default/OrchidUI/，示例扩展。\n构建脚本位于 scripts/build.py，产物写入受控输出目录。\n"),
    ("0002.mem", "@2 skill published I2 T2 2026-07-20\n= 部署, SafeDeploy\n~ self:opencode\n部署 OrchidUI 改动的标准流程。\n1. 签名构建 → 2. 经 SafeDeploy 部署\n! 禁止绕过 SafeDeploy 直接调用底层设备接口\n"),
    ("0003.mem", "@3 fact published I3 T3 2026-08-02\n= OrchidUI Web 当前 CEF 运行组件下载地址\n~ user\n# lock\nOrchidUI Web 当前使用示例下载域名 downloads.example.com。\nMac 与 Windows 的 CEF 组件地址由 current.json 清单提供。\n"),
    ("0004.mem", "@4 fact candidate I2 T2 2026-08-01\n= 待审\n~ self:agent\n# scope project:demo\n候选卡片不参与默认检索。\n"),
    ("0005.mem", "@5 fact published I1 T1 2024-01-01\n~ external:import\n外部来源的低可信旧卡片，提及 OrchidUI 与 downloads.example.com。\n"),
    ("0006.mem", "@6 fact published I2 T2 2026-06-15\n= 记忆宫殿\n~ user\n# scope project:demo\n项目内可见的记忆宫殿构建方法。\n"),
    ("0007.mem", "@7 fact published I2 T2.5 2026-05-10\n= 东京塔\n~ user\n# last-used 2026-08-01\n# valid-from 2026-01-01\n# valid-to 2026-12-31\n东京塔高度 333 米。\n"),
    ("0008.mem", "@8 fact published I2 T2 2026-05-10\n~ user\n# supersedes @7\n新东京塔数据。\n"),
    ("0009.mem", "@9 fact published I2 T2 2026-03-01\n~ user\n# valid-to 2026-01-01\n过期的卡片不应出现。\n"),
    ("0010.mem", "@10 fact archived I3 T3 2026-02-01\n= OrchidUI\n~ user\n归档的 OrchidUI 备注。\n"),
    ("0011.mem", "@11 decision published I1 T2 2026-04-01\n= NexusCore2\n~ user\nNexusCore2 模块负责检索编排，OrchidUI 依赖它。\n"),
];

fn corpus_home() -> PathBuf {
    std::env::set_var("ENGRAMARK_TEST_NOW", FAKE_NOW);
    let unique = format!(
        "engramark-golden-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let dir = std::env::temp_dir().join(unique);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("cards")).expect("mkdir");
    for (name, text) in CORPUS {
        std::fs::write(dir.join("cards").join(name), text).expect("write card");
    }
    std::fs::create_dir_all(dir.join("state")).expect("mkdir state");
    std::fs::write(dir.join("state").join("id-sequence"), "11\n").expect("sequence");
    dir
}

fn corpus_layout() -> Layout {
    Layout {
        home: corpus_home(),
    }
}

fn corpus_cards(layout: &Layout) -> Vec<engramark::mem::Card> {
    CORPUS
        .iter()
        .map(|(name, _)| {
            engramark::cache::load_card_file(&layout.cards().join(name)).expect("card")
        })
        .collect()
}

#[test]
fn radar_golden() {
    let doc = load("radar.json");
    let cfg = default_config();
    let layout = corpus_layout();
    let cards = corpus_cards(&layout);
    let blob = engramark::radar::build_radar_blob(&cards, &cfg);
    let expected_hex = doc
        .get("blob_hex")
        .and_then(Json::as_str)
        .expect("blob_hex");
    let actual_hex: String = blob.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(actual_hex, expected_hex, "radar blob bytes");
    let (anchors, ac) = engramark::radar::decode_radar_blob(&blob).expect("decode");
    for scan in doc.get("scans").and_then(Json::as_array).expect("scans") {
        let text = scan.get("text").and_then(Json::as_str).expect("text");
        let project = scan.get("project").and_then(Json::as_str).expect("project");
        let hits =
            engramark::radar::radar_hits_from_runtime(&anchors, &ac, text, project, None, None)
                .expect("hits");
        let expected = scan.get("hits").and_then(Json::as_array).expect("hits");
        assert_eq!(hits.len(), expected.len(), "hits({text:?} / {project})");
        for (actual, expected) in hits.iter().zip(expected) {
            assert_eq!(
                actual.anchor,
                expected.get("anchor").and_then(Json::as_str).unwrap()
            );
            assert_eq!(
                actual.entity,
                expected.get("entity").and_then(Json::as_str).unwrap()
            );
            assert_eq!(
                actual.id,
                expected.get("id").and_then(Json::as_i64).unwrap()
            );
            assert_eq!(
                actual.strength(),
                expected.get("strength").and_then(Json::as_str).unwrap()
            );
            assert_eq!(
                actual.kind,
                expected.get("kind").and_then(Json::as_str).unwrap()
            );
            assert_eq!(
                actual.manual,
                expected.get("manual").and_then(Json::as_bool).unwrap()
            );
            assert_eq!(
                actual.scope,
                expected.get("scope").and_then(Json::as_str).unwrap()
            );
        }
    }
}

#[test]
fn search_golden() {
    let cases = load("search.json");
    let layout = corpus_layout();
    engramark::cache::rebuild(&layout).expect("rebuild");
    let cfg = default_config();
    for case in cases.as_array().expect("array") {
        let query = case.get("query").and_then(Json::as_str).expect("query");
        let project = case.get("project").and_then(Json::as_str).expect("project");
        if case.get("top_limit").is_some() {
            let rows = engramark::search::search(&layout, "", "published", 3, project, Some(&cfg))
                .expect("top search");
            let lines: Vec<String> = rows
                .iter()
                .map(|row| engramark::textops::index_line(row, false))
                .collect();
            let expected: Vec<String> = case
                .get("lines")
                .and_then(Json::as_array)
                .expect("lines")
                .iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect();
            assert_eq!(lines, expected, "top lines");
            let human: Vec<String> = rows
                .iter()
                .map(engramark::textops::human_index_line)
                .collect();
            let expected_human: Vec<String> = case
                .get("human_lines")
                .and_then(Json::as_array)
                .expect("human_lines")
                .iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect();
            assert_eq!(human, expected_human, "top human lines");
            continue;
        }
        let rows = engramark::search::search(&layout, query, "published", 8, project, Some(&cfg))
            .expect("search");
        let expected_rows = case.get("rows").and_then(Json::as_array).expect("rows");
        assert_eq!(
            rows.len(),
            expected_rows.len(),
            "rows({query:?} / {project})"
        );
        for (actual, expected) in rows.iter().zip(expected_rows) {
            compare_row(actual, expected, query);
        }
        let lines: Vec<String> = rows
            .iter()
            .map(|row| engramark::textops::index_line(row, false))
            .collect();
        let expected_lines: Vec<String> = case
            .get("lines")
            .and_then(Json::as_array)
            .expect("lines")
            .iter()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect();
        assert_eq!(lines, expected_lines, "lines({query:?})");
        let human: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(pos, row)| {
                engramark::textops::human_search_line(
                    row,
                    pos,
                    engramark::config::section(&cfg, "search"),
                )
            })
            .collect();
        let expected_human: Vec<String> = case
            .get("human_lines")
            .and_then(Json::as_array)
            .expect("human_lines")
            .iter()
            .filter_map(|l| l.as_str().map(str::to_string))
            .collect();
        assert_eq!(human, expected_human, "human lines({query:?})");
    }
}

fn compare_row(actual: &engramark::textops::MetaRow, expected: &Json, query: &str) {
    let str_field = |key: &str| expected.get(key).and_then(Json::as_str).unwrap_or("");
    assert_eq!(
        actual.id,
        expected.get("id").and_then(Json::as_i64).unwrap(),
        "{query:?}"
    );
    assert_eq!(actual.status, str_field("status"));
    assert_eq!(actual.card_type, str_field("type"));
    assert_eq!(actual.i, expected.get("i").and_then(Json::as_i64).unwrap());
    assert_eq!(actual.t, expected.get("t").and_then(Json::as_i64).unwrap());
    assert_eq!(actual.last_used, str_field("last_used"));
    assert_eq!(actual.updated, str_field("updated"));
    assert_eq!(actual.source, str_field("source"));
    assert_eq!(
        actual.lock,
        expected.get("lock").and_then(Json::as_i64).unwrap_or(0) == 1
            || expected
                .get("lock")
                .and_then(Json::as_bool)
                .unwrap_or(false)
    );
    assert_eq!(actual.scope, str_field("scope"));
    assert_eq!(actual.title, str_field("title"));
    assert_eq!(actual.body, str_field("body"));
    assert_eq!(actual.entities, str_field("entities"));
    assert_eq!(actual.valid_from, str_field("valid_from"));
    assert_eq!(actual.valid_to, str_field("valid_to"));
    assert_eq!(actual.supersedes, str_field("supersedes"));
    assert_eq!(actual.semantic_hash, str_field("semantic_hash"));
    assert_eq!(actual.source_hash, str_field("source_hash"));
    if let Some(score) = expected.get("_score").and_then(Json::as_f64) {
        assert_eq!(actual.score, score, "score({query:?})");
    }
    assert_eq!(
        actual.evidence,
        str_field("_evidence"),
        "evidence({query:?})"
    );
    if let Some(strong) = expected.get("_strong_anchor").and_then(Json::as_bool) {
        assert_eq!(actual.strong_anchor, strong);
    }
    if let Some(rrf) = expected.get("_rrf").and_then(Json::as_f64) {
        assert_eq!(actual.rrf, rrf, "rrf({query:?})");
    }
    assert_eq!(
        actual.confidence,
        str_field("_confidence"),
        "confidence({query:?})"
    );
}

#[test]
fn text_ops_golden() {
    let cases = load("text_ops.json");
    for case in cases.as_array().expect("array") {
        let name = case.get("name").and_then(Json::as_str).expect("name");
        match name {
            "index-line" | "index-line-explain" | "radar-line" => {
                // Rendering covered by search golden and radar line unit below.
            }
            _ => {
                let input = case.get("input").and_then(Json::as_str).expect("input");
                let output = case.get("output").and_then(Json::as_str).expect("output");
                let actual = if name.starts_with("truncate-") {
                    let (codepoints, bytes) = truncate_args(name);
                    engramark::textops::truncate_text(input, codepoints, bytes, "…")
                } else if name.starts_with("excerpt-") {
                    let (codepoints, bytes, first) = excerpt_args(name);
                    engramark::textops::memory_excerpt(input, codepoints, bytes, first)
                } else {
                    engramark::textops::human_display_title(input, 160)
                };
                assert_eq!(actual, output, "{name}");
            }
        }
    }
    // Dedicated rendering cases embedded in the fixture doc below.
    let metas = [
        meta_row(3, "OrchidUI Web 当前 CEF 运行组件下载地址",
            "OrchidUI Web 当前使用示例下载域名 downloads.example.com。\nMac 与 Windows 的 CEF 组件地址由 current.json 清单提供。",
            3, 6, "2026-08-02", "2026-08-02", ""),
        meta_row(5, "外部来源的低可信旧卡片", "", 1, 2, "", "2024-01-01", "medium"),
    ];
    let index_lines: Vec<String> = metas
        .iter()
        .map(|row| engramark::textops::index_line(row, false))
        .collect();
    assert_eq!(index_lines.len(), 2);
    let radar_lines: Vec<String> = vec![
        engramark::textops::human_radar_line(&metas[0], "downloads.example.com", 120),
        engramark::textops::human_radar_line(&metas[1], "", 120),
        engramark::textops::human_radar_line(&metas[0], &"x".repeat(200), 120),
    ];
    let expected_radar: Vec<String> = radar_line_expectations(&cases);
    if !expected_radar.is_empty() {
        assert_eq!(radar_lines, expected_radar, "radar lines");
    }
    let expected_index: Vec<String> = index_line_expectations(&cases);
    if !expected_index.is_empty() {
        assert_eq!(index_lines, expected_index, "index lines");
    }
    let explain_row = {
        let mut row = metas[0].clone();
        row.evidence = "强锚点、原短语".into();
        row.score = 0.87;
        row
    };
    let explain = vec![engramark::textops::index_line(&explain_row, true)];
    let expected_explain = explain_expectations(&cases);
    if !expected_explain.is_empty() {
        assert_eq!(explain, expected_explain, "explain line");
    }
}

#[allow(clippy::too_many_arguments)]
fn meta_row(
    id: i64,
    title: &str,
    body: &str,
    i: i64,
    t: i64,
    last_used: &str,
    updated: &str,
    confidence: &str,
) -> engramark::textops::MetaRow {
    engramark::textops::MetaRow {
        id,
        title: title.into(),
        body: body.into(),
        card_type: "fact".into(),
        i,
        t,
        last_used: last_used.into(),
        updated: updated.into(),
        confidence: confidence.into(),
        ..Default::default()
    }
}

fn case_output(cases: &Json, name: &str) -> Vec<String> {
    cases
        .as_array()
        .unwrap_or(&[])
        .iter()
        .find(|case| case.get("name").and_then(Json::as_str) == Some(name))
        .and_then(|case| case.get("output").and_then(Json::as_array))
        .map(|items| {
            items
                .iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn radar_line_expectations(cases: &Json) -> Vec<String> {
    case_output(cases, "radar-line")
}

fn index_line_expectations(cases: &Json) -> Vec<String> {
    case_output(cases, "index-line")
}

fn explain_expectations(cases: &Json) -> Vec<String> {
    case_output(cases, "index-line-explain")
}

fn truncate_args(name: &str) -> (Option<usize>, Option<usize>) {
    match name {
        "truncate-simple" => (Some(5), None),
        "truncate-bytes" => (None, Some(10)),
        "truncate-both" => (Some(4), Some(8)),
        "truncate-exact" => (Some(5), None),
        "truncate-zero" => (Some(0), None),
        _ => (None, None),
    }
}

fn excerpt_args(name: &str) -> (Option<usize>, Option<usize>, bool) {
    match name {
        "excerpt-long" => (None, Some(200), false),
        "excerpt-unicode" => (None, Some(120), false),
        "excerpt-paragraph" => (Some(100), None, true),
        "excerpt-gist" => (Some(30), None, true),
        "excerpt-control" => (Some(20), None, false),
        _ => (None, None, false),
    }
}

#[test]
fn errors_golden() {
    let cases = load("errors.json");
    for case in cases.as_array().expect("array") {
        let name = case.get("name").and_then(Json::as_str).expect("name");
        let expected = case.get("error").and_then(Json::as_str).expect("error");
        let actual: engramark::Result<()> = match name {
            "title-empty" => {
                engramark::mem::normalize_structured_content("  ", "", &[], "fact").map(|_| ())
            }
            "title-long" => {
                engramark::mem::normalize_structured_content(&"x".repeat(121), "", &[], "fact")
                    .map(|_| ())
            }
            "title-newline" => {
                engramark::mem::normalize_structured_content("a\nb", "", &[], "fact").map(|_| ())
            }
            "body-nul" => {
                engramark::mem::normalize_structured_content("t", "a\0b", &[], "fact").map(|_| ())
            }
            "entity-comma" => {
                engramark::mem::normalize_structured_content("t", "", &["a,b".into()], "fact")
                    .map(|_| ())
            }
            "entity-long" => {
                engramark::mem::normalize_structured_content("t", "", &["x".repeat(129)], "fact")
                    .map(|_| ())
            }
            "bad-type" => {
                engramark::mem::normalize_structured_content("t", "", &[], "unknown").map(|_| ())
            }
            "scope-bad" => engramark::mem::stored_scope("weird", "demo").map(|_| ()),
            "scope-project-global" => engramark::mem::stored_scope("project", "global").map(|_| ()),
            _ => panic!("unknown case {name}"),
        };
        match actual {
            Err(err) => assert_eq!(err.to_string(), expected, "{name}"),
            Ok(_) => panic!("{name}: expected error"),
        }
    }
    assert_eq!(
        engramark::mem::stored_scope("weird", "demo")
            .unwrap_err()
            .to_string(),
        "适用范围只能是 global 或 project"
    );
    assert_eq!(
        engramark::mem::stored_scope("project", "global")
            .unwrap_err()
            .to_string(),
        "scope=project 需要可识别的项目目录；请在项目会话中重试，或明确使用 global"
    );
}

#[test]
fn scan_golden() {
    let doc = load("scan.json");
    let layout = corpus_layout();
    engramark::cache::rebuild(&layout).expect("rebuild");
    let first =
        engramark::hooks::scan_text(&layout, "OrchidUI 的下载地址？", "golden", None, "global")
            .expect("scan");
    let expected_first = doc.get("first").expect("first");
    compare_scan(&first, expected_first);
    let second =
        engramark::hooks::scan_text(&layout, "OrchidUI 的下载地址？", "golden", None, "global")
            .expect("scan");
    let expected_second = doc.get("second_cooldown").expect("second_cooldown");
    compare_scan(&second, expected_second);
    let _ = std::fs::remove_dir_all(&layout.home);
}

fn compare_scan(actual: &Json, expected: &Json) {
    assert_eq!(
        actual.get("lines").and_then(Json::as_array),
        expected.get("lines").and_then(Json::as_array),
        "scan lines"
    );
    assert_eq!(
        actual.get("hits").and_then(Json::as_array),
        expected.get("hits").and_then(Json::as_array),
        "scan hits"
    );
    assert_eq!(
        actual.get("context").and_then(Json::as_str),
        expected.get("context").and_then(Json::as_str),
        "scan context"
    );
}
