//! Query planning (QUERY_PLANNER v3): tokens, weights, generic/strong flags.

use std::collections::BTreeSet;

use crate::config;
use crate::json::Json;
use crate::normalize::{contains_cjk, normalize_text, py_len};
use crate::pyregex;

#[derive(Clone, Debug, PartialEq)]
pub struct QueryTerm {
    pub text: String,
    pub norm: String,
    pub weight: f64,
    pub generic: bool,
    pub strong: bool,
}

#[derive(Clone, Debug)]
pub struct QueryPlan {
    pub raw: String,
    pub norm: String,
    pub terms: Vec<QueryTerm>,
}

pub const CONTENT_INTENT_TERMS: &[&str] = &[
    "地址", "下载", "服务", "文件", "配置", "部署", "构建", "清单", "address", "download",
    "service", "file", "config", "deploy", "build", "manifest", "url",
];

pub fn plan_query(query: &str, cfg: &Json) -> QueryPlan {
    let search = config::section(cfg, "search");
    let generic: BTreeSet<String> = config::string_list(config::get(search, "generic_terms"))
        .iter()
        .map(|t| normalize_text(t))
        .collect();
    let weak: BTreeSet<String> = config::string_list(config::get(search, "weak_anchors"))
        .iter()
        .map(|t| normalize_text(t))
        .collect();
    // QUERY_TOKEN_RE runs over NFKC(query), not the casefolded form.
    let nfkc: String = unicode_normalization::UnicodeNormalization::nfkc(query).collect();
    let raw_tokens: Vec<String> = pyregex::find_query_tokens(&nfkc)
        .into_iter()
        .map(|(start, end)| nfkc[start..end].to_string())
        .collect();
    let mut expanded: Vec<String> = Vec::new();
    for token in &raw_tokens {
        expanded.push(
            token
                .trim_end_matches(['.', ',', ';', '，', '。', '；'])
                .to_string(),
        );
        if contains_cjk(token) && py_len(token) > 2 {
            let normalized_token = normalize_text(token);
            for g in &generic {
                if py_len(g) >= 2 && normalized_token.contains(g.as_str()) {
                    expanded.push(g.clone());
                }
            }
        }
    }
    let mut terms: Vec<QueryTerm> = Vec::new();
    for token in &expanded {
        let norm = normalize_text(token);
        if py_len(&norm) < 2 || terms.iter().any(|t: &QueryTerm| t.norm == norm) {
            continue;
        }
        let is_generic = generic.contains(&norm);
        let acronym = pyregex::is_caps_identifier(token);
        let code_like = acronym || pyregex::is_url_full(token) || pyregex::is_domain_full(token);
        let strong = code_like && !weak.contains(&norm);
        let weight = if strong {
            2.5
        } else if is_generic {
            0.35
        } else {
            1.0
        };
        terms.push(QueryTerm {
            text: token.clone(),
            norm,
            weight,
            generic: is_generic,
            strong,
        });
    }
    QueryPlan {
        raw: query.to_string(),
        norm: normalize_text(query),
        terms,
    }
}
