//! Unit cases migrated from the Python module-import tests (test_core [5]).

use engramark::json::Json;
use engramark::textops::{
    human_index_line, human_radar_line, human_search_line, memory_excerpt, MetaRow,
};
use engramark::{HOOK_BLOCK_PREFIX, HOOK_BLOCK_SUFFIX, HOOK_MAX_BLOCK_BYTES};

fn meta(id: i64, title: &str, body: &str, confidence: &str) -> MetaRow {
    MetaRow {
        id,
        title: title.into(),
        body: body.into(),
        card_type: "fact".into(),
        i: 3,
        t: 6,
        last_used: "2026-08-02".into(),
        updated: "2026-08-02".into(),
        confidence: confidence.into(),
        ..Default::default()
    }
}

#[test]
fn utf8_excerpt_keeps_codepoints() {
    let excerpt = memory_excerpt(&"😀".repeat(20), None, Some(10), false);
    assert!(excerpt.len() <= 10 && excerpt.ends_with('…'), "{excerpt:?}");
}

#[test]
fn excerpt_collapses_whitespace_and_first_paragraph() {
    let excerpt = memory_excerpt(
        " \r\n 首\t段\u{7}事实 \r\n\r\n 第二段",
        Some(20),
        None,
        true,
    );
    assert_eq!(excerpt, "首 段 事实");
}

#[test]
fn combining_characters_respect_both_limits() {
    let excerpt = memory_excerpt(&"e\u{301}".repeat(10), Some(5), Some(6), false);
    assert!(excerpt.chars().count() <= 5 && excerpt.len() <= 6 && excerpt.ends_with('…'));
}

#[test]
fn summary_keeps_exact_160_codepoints() {
    let exact = human_index_line(&meta(91, "边界", &"x".repeat(160), "high"));
    let long = human_index_line(&meta(92, "边界", &"x".repeat(161), "high"));
    assert!(exact.ends_with(&"x".repeat(160)), "{exact}");
    assert!(long.ends_with(&format!("{}…", "x".repeat(159))), "{long}");
}

#[test]
fn radar_line_first_paragraph_only() {
    let line = human_radar_line(
        &meta(93, "段落", "首段事实\r\n\r\n第二段不应注入", ""),
        "段落",
        120,
    );
    assert!(
        line.contains("首段事实") && !line.contains("第二段"),
        "{line}"
    );
}

#[test]
fn radar_line_handles_oversized_titles() {
    // 标题被 human_display_title 截断到 160 码点，必需部分永不超行限；
    // 超限时整卡跳过的防御分支在 Python 侧同样不可达。
    let long_title = "必".repeat(400);
    let line = human_radar_line(&meta(93, &long_title, "正文", ""), "标题", 120);
    assert!(
        line.starts_with("记忆提示：记忆 93：") && line.len() <= 900,
        "{line}"
    );
}

#[test]
fn packing_skips_long_items_and_keeps_short_ones() {
    let candidates = vec![
        engramark::jobject! {"id" => 1i64, "line" => "a".repeat(800)},
        engramark::jobject! {"id" => 2i64, "line" => "b".repeat(300)},
        engramark::jobject! {"id" => 3i64, "line" => "c".repeat(40)},
    ];
    let packed = engramark::hooks::pack_radar_candidates(
        &candidates,
        3,
        HOOK_BLOCK_PREFIX,
        HOOK_BLOCK_SUFFIX,
    );
    let ids: Vec<i64> = packed
        .iter()
        .filter_map(|item| item.get("id").and_then(Json::as_i64))
        .collect();
    assert_eq!(ids, vec![1, 3]);
    let lines: Vec<String> = packed
        .iter()
        .filter_map(|item| item.get("line").and_then(Json::as_str).map(str::to_string))
        .collect();
    assert!(
        engramark::hooks::radar_block_size(&lines, HOOK_BLOCK_PREFIX, HOOK_BLOCK_SUFFIX)
            <= HOOK_MAX_BLOCK_BYTES
    );
}

#[test]
fn preview_rules() {
    let fake = meta(
        94,
        "预览边界",
        &format!("回答事实 {}", "😀".repeat(50)),
        "high",
    );
    let cfg = Json::parse(r#"{"preview_enabled": true, "preview_max_bytes": 40}"#).unwrap();
    let line = human_search_line(&fake, 0, Some(&cfg));
    let preview = line.split("正文预览：").nth(1).expect("preview");
    assert!(preview.len() <= 40 && preview.ends_with('…'), "{line}");

    let exact = meta(94, "预览边界", &"x".repeat(40), "high");
    let exact_line = human_search_line(&exact, 0, Some(&cfg));
    assert!(
        exact_line.ends_with(&"x".repeat(40)) && !exact_line.ends_with('…'),
        "{exact_line}"
    );

    let empty = meta(94, "预览边界", "", "high");
    assert!(!human_search_line(&empty, 0, Some(&cfg)).contains("正文预览："));

    assert!(human_search_line(&fake, 1, Some(&cfg)).contains("摘要："));

    let disabled = Json::parse(r#"{"preview_enabled": false, "preview_max_bytes": 40}"#).unwrap();
    assert!(human_search_line(&fake, 0, Some(&disabled)).contains("摘要："));

    let medium = meta(
        94,
        "预览边界",
        &format!("回答事实 {}", "😀".repeat(50)),
        "medium",
    );
    assert!(!human_search_line(&medium, 0, Some(&cfg)).contains("正文预览："));
}
