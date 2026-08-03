//! engramark.json loading: frozen merge, fallback and clamping semantics.
//! An unreadable or non-object file falls back to defaults entirely; present
//! sections are shallow-merged over the default section.

use std::path::Path;

use crate::json::Json;

pub fn default_config() -> Json {
    Json::parse(DEFAULT_CONFIG_JSON).expect("embedded default config parses")
}

const DEFAULT_CONFIG_JSON: &str = include_str!("../../engramark.json");

pub fn load_config(home: &Path) -> Json {
    let path = home.join("engramark.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return default_config();
    };
    let Ok(parsed) = Json::parse(&text) else {
        return default_config();
    };
    let Some(sections) = parsed.as_object() else {
        return default_config();
    };
    let mut merged = default_config();
    for (key, value) in sections {
        if value.is_object() && merged.get(key).is_some_and(|base| base.is_object()) {
            if let Some(base_pairs) = merged.get(key).and_then(Json::as_object) {
                let mut updated: Vec<(String, Json)> = base_pairs.to_vec();
                if let Json::Object(overrides) = value {
                    for (sub_key, sub_value) in overrides {
                        if let Some(slot) = updated.iter_mut().find(|(k, _)| k == sub_key) {
                            slot.1 = sub_value.clone();
                        } else {
                            updated.push((sub_key.clone(), sub_value.clone()));
                        }
                    }
                }
                if let Json::Object(ref mut merged_pairs) = merged {
                    if let Some(slot) = merged_pairs.iter_mut().find(|(k, _)| *k == *key) {
                        slot.1 = Json::Object(updated);
                    }
                }
            }
        } else if let Json::Object(ref mut merged_pairs) = merged {
            if let Some(slot) = merged_pairs.iter_mut().find(|(k, _)| *k == *key) {
                slot.1 = value.clone();
            } else {
                merged_pairs.push((key.clone(), value.clone()));
            }
        }
    }
    merged
}

pub fn section<'a>(config: &'a Json, name: &str) -> Option<&'a Json> {
    config.get(name).filter(|value| value.is_object())
}

pub fn get<'a>(section: Option<&'a Json>, key: &str) -> Option<&'a Json> {
    section.and_then(|value| value.get(key))
}

/// Python int(value): bool → 0/1, float → truncate toward zero, str → parse.
pub fn py_int(value: &Json) -> Option<i64> {
    match value {
        Json::Bool(v) => Some(if *v { 1 } else { 0 }),
        Json::Int(v) => Some(*v),
        Json::Float(v) if v.is_finite() && v.abs() < 9.0e18 => Some(v.trunc() as i64),
        Json::Str(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.parse::<i64>().ok()
        }
        _ => None,
    }
}

/// Python float(value) for config numbers.
pub fn py_float(value: &Json) -> Option<f64> {
    match value {
        Json::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        Json::Int(v) => Some(*v as f64),
        Json::Float(v) => Some(*v),
        Json::Str(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// _bounded_int: int() with fallback, then clamp into [minimum, maximum].
pub fn bounded_int(value: Option<&Json>, default: i64, minimum: i64, maximum: i64) -> i64 {
    let number = value.and_then(py_int).unwrap_or(default);
    number.clamp(minimum, maximum)
}

/// String list from config, preserving file order.
pub fn string_list(value: Option<&Json>) -> Vec<String> {
    match value {
        Some(Json::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}
