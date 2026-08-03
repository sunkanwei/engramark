//! Python-compatible JSON model and encoders.
//!
//! Three renderings exist, matching the frozen Python call sites:
//! - canonical: sort_keys + compact separators, used by journal and radar blob
//!   hashing (MEMTXN/MRDR) and state files;
//! - default: insertion order with ", " / ": " separators (json.dumps default);
//! - indent1: json.dumps(indent=1) for audit/diagnose output.
//!
//! Numbers stay type-sensitive: `1` and `1.0` are distinct values, exactly as
//! Python's int/float split. Floats render with CPython repr rules.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        Ok(from_serde(value))
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(v) => Some(*v as f64),
            Json::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(pairs) => Some(pairs),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Object(_))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn keys(&self) -> Vec<&str> {
        match self {
            Json::Object(pairs) => pairs.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    /// Canonical encoding: sorted keys, compact separators. Used wherever the
    /// bytes feed a SHA-256 or a state file shared with the Python reference.
    pub fn dumps_canonical(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Mode::Canonical, 0);
        out
    }

    /// json.dumps(ensure_ascii=False) default separators.
    pub fn dumps(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Mode::Default, 0);
        out
    }

    /// json.dumps(ensure_ascii=False, indent=2) — insertion order.
    pub fn dumps_indent2(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Mode::Indent2, 0);
        out
    }

    /// json.dumps(ensure_ascii=False, sort_keys=True, indent=2) — snapshot manifest.
    pub fn dumps_indent2_sorted(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Mode::Indent2Sorted, 0);
        out
    }

    /// json.dumps(ensure_ascii=False, indent=1).
    pub fn dumps_indent1(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out, Mode::Indent1, 0);
        out
    }
}

fn from_serde(value: serde_json::Value) -> Json {
    match value {
        serde_json::Value::Null => Json::Null,
        serde_json::Value::Bool(v) => Json::Bool(v),
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                Json::Int(v)
            } else if let Some(v) = n.as_u64() {
                Json::Float(v as f64)
            } else {
                Json::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Json::Str(s),
        serde_json::Value::Array(items) => Json::Array(items.into_iter().map(from_serde).collect()),
        serde_json::Value::Object(map) => {
            Json::Object(map.into_iter().map(|(k, v)| (k, from_serde(v))).collect())
        }
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Json {
        Json::Str(value.to_string())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Json {
        Json::Str(value)
    }
}

impl From<&String> for Json {
    fn from(value: &String) -> Json {
        Json::Str(value.clone())
    }
}

impl From<std::borrow::Cow<'_, str>> for Json {
    fn from(value: std::borrow::Cow<'_, str>) -> Json {
        Json::Str(value.into_owned())
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Json {
        Json::Int(value)
    }
}

impl From<usize> for Json {
    fn from(value: usize) -> Json {
        Json::Int(value as i64)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Json {
        Json::Bool(value)
    }
}

impl From<f64> for Json {
    fn from(value: f64) -> Json {
        Json::Float(value)
    }
}

impl From<Vec<Json>> for Json {
    fn from(value: Vec<Json>) -> Json {
        Json::Array(value)
    }
}

impl From<Vec<(String, Json)>> for Json {
    fn from(value: Vec<(String, Json)>) -> Json {
        Json::Object(value)
    }
}

#[macro_export]
macro_rules! jobject {
    ($($key:expr => $value:expr),* $(,)?) => {
        $crate::json::Json::Object(vec![$(($key.to_string(), $crate::json::Json::from($value))),*])
    };
}

pub fn escape_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// CPython repr() for a finite double: shortest round-trip decimal, fixed
/// notation for exponents in [-4, 16), scientific with signed 2-digit exponent
/// otherwise.
pub fn float_repr(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let scientific = format!("{:e}", value);
    let (mantissa, exponent) = scientific.split_once('e').expect("rust {:e} format");
    let exponent: i32 = exponent.parse().expect("rust {:e} exponent");
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let mut out = String::from(sign);
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..-point {
                out.push('0');
            }
            out.push_str(digits);
        } else if (point as usize) >= digits.len() {
            out.push_str(digits);
            for _ in 0..(point as usize - digits.len()) {
                out.push('0');
            }
            out.push_str(".0");
        } else {
            out.push_str(&digits[..point as usize]);
            out.push('.');
            out.push_str(&digits[point as usize..]);
        }
        out
    } else {
        let mut out = String::from(sign);
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let marker = if exponent < 0 { "e-" } else { "e+" };
        out.push_str(&format!("{}{:02}", marker, exponent.abs()));
        out
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Canonical,
    Default,
    Indent1,
    Indent2,
    Indent2Sorted,
}

fn write_value(value: &Json, out: &mut String, mode: Mode, depth: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(v) => out.push_str(&v.to_string()),
        Json::Float(v) => out.push_str(&float_repr(*v)),
        Json::Str(s) => escape_string(s, out),
        Json::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    match mode {
                        Mode::Canonical => out.push(','),
                        Mode::Default => out.push_str(", "),
                        Mode::Indent1 | Mode::Indent2 | Mode::Indent2Sorted => out.push(','),
                    }
                }
                if mode == Mode::Indent1 {
                    out.push('\n');
                    out.push_str(&" ".repeat(depth + 1));
                }
                if mode == Mode::Indent2 || mode == Mode::Indent2Sorted {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                }
                write_value(item, out, mode, depth + 1);
            }
            if mode == Mode::Indent1 {
                out.push('\n');
                out.push_str(&" ".repeat(depth));
            }
            if mode == Mode::Indent2 || mode == Mode::Indent2Sorted {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
            out.push(']');
        }
        Json::Object(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let mut ordered: Vec<&(String, Json)> = pairs.iter().collect();
            if mode == Mode::Canonical || mode == Mode::Indent2Sorted {
                ordered.sort_by(|a, b| a.0.cmp(&b.0));
            }
            for (index, (key, item)) in ordered.iter().enumerate() {
                if index > 0 {
                    match mode {
                        Mode::Canonical => out.push(','),
                        Mode::Default => out.push_str(", "),
                        Mode::Indent1 | Mode::Indent2 | Mode::Indent2Sorted => out.push(','),
                    }
                }
                if mode == Mode::Indent1 {
                    out.push('\n');
                    out.push_str(&" ".repeat(depth + 1));
                }
                if mode == Mode::Indent2 || mode == Mode::Indent2Sorted {
                    out.push('\n');
                    out.push_str(&"  ".repeat(depth + 1));
                }
                escape_string(key, out);
                out.push_str(if mode == Mode::Canonical { ":" } else { ": " });
                write_value(item, out, mode, depth + 1);
            }
            if mode == Mode::Indent1 {
                out.push('\n');
                out.push_str(&" ".repeat(depth));
            }
            if mode == Mode::Indent2 || mode == Mode::Indent2Sorted {
                out.push('\n');
                out.push_str(&"  ".repeat(depth));
            }
            out.push('}');
        }
    }
}
