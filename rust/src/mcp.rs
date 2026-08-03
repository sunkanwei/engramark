//! MCP server over stdio NDJSON JSON-RPC: 11 tools, protocol versions
//! 2025-06-18 and 2025-11-25, roots/list round-trip, sanitized logging.
//! stdout carries protocol frames only; logs go to the log file. Requests are
//! executed sequentially; frames are capped at 16 MiB with a bounded drain.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use crate::json::Json;
use crate::normalize::py_len;
use crate::paths::{project_directory, project_id, Layout};
use crate::textops::{human_display_title, human_search_line, humanize_memory_text};
use crate::{
    cache, config, lifecycle, search, Error, Result, GET_MAX_IDS, MAX_CARD_BYTES, MAX_ENTITIES,
    MAX_ENTITY_CHARS, MAX_PUBLIC_ID, MAX_QUERY_CHARS, MAX_TITLE_CHARS, VERSION,
};

const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];
const LATEST_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];
const SERVER_NAME: &str = "engramark";
const SERVER_INSTRUCTIONS: &str = "只有用户明确表达长期保存意图时才使用 memory_save；不限定关键词或语言，按语义识别，例如记住、记一下、以后默认、下次别忘、remember this、save this for later、make a note、from now on、don't forget 等自然说法，保存后只需简短确认。不得因为信息看起来有价值而主动保存或询问。只有用户明确要求先存为候选时才使用 memory_propose。先 memory_search，再按需 memory_get。只有确有对错证据时才 memory_feedback。保存和提议必须明确 scope；project 仅用于已有可靠项目上下文的记忆。";
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

fn id_schema() -> Json {
    crate::jobject! {
        "type" => "integer",
        "minimum" => 1i64,
        "maximum" => MAX_PUBLIC_ID,
        "description" => "正整数记忆编号",
    }
}

fn title_schema() -> Json {
    crate::jobject! {
        "type" => "string",
        "minLength" => 1i64,
        "maxLength" => MAX_TITLE_CHARS as i64,
        "pattern" => "^[^\\r\\n\\u0000]+$",
        "description" => "单行、自足、无需上下文也能理解的标题",
    }
}

fn body_schema() -> Json {
    crate::jobject! {
        "type" => "string",
        "maxLength" => MAX_CARD_BYTES as i64,
        "default" => "",
        "description" => "可选正文；使用自然语言，可包含段落",
    }
}

fn entities_schema() -> Json {
    crate::jobject! {
        "type" => "array",
        "items" => crate::jobject! {
            "type" => "string",
            "minLength" => 1i64,
            "maxLength" => MAX_ENTITY_CHARS as i64,
            "pattern" => "^[^,\\r\\n\\u0000]+$",
        },
        "maxItems" => MAX_ENTITIES as i64,
        "uniqueItems" => true,
        "default" => Json::Array(Vec::new()),
        "description" => "用于检索的实体名称；不要放句子、逗号或换行",
    }
}

fn type_schema() -> Json {
    crate::jobject! {
        "type" => "string",
        "enum" => Json::Array(vec!["fact".into(), "decision".into(), "skill".into()]),
        "default" => "fact",
        "description" => "记忆类型：事实、决策或可复用流程",
    }
}

fn scope_schema() -> Json {
    crate::jobject! {
        "type" => "string",
        "enum" => Json::Array(vec!["global".into(), "project".into()]),
        "description" => "global 适用于所有项目；project 只适用于当前项目",
    }
}

fn object_schema(
    properties: Vec<(String, Json)>,
    required: Option<Vec<&str>>,
    extra: Vec<(String, Json)>,
) -> Json {
    let mut schema = vec![
        ("type".to_string(), Json::Str("object".into())),
        ("properties".to_string(), Json::Object(properties)),
        ("additionalProperties".to_string(), Json::Bool(false)),
    ];
    if let Some(required) = required {
        if !required.is_empty() {
            schema.push((
                "required".to_string(),
                Json::Array(required.into_iter().map(Json::from).collect()),
            ));
        }
    }
    schema.extend(extra);
    Json::Object(schema)
}

fn annotations(title: &str, read_only: bool, destructive: bool, idempotent: bool) -> Json {
    crate::jobject! {
        "title" => title,
        "readOnlyHint" => read_only,
        "destructiveHint" => destructive,
        "idempotentHint" => idempotent,
        "openWorldHint" => false,
    }
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    schema: Json,
    read_only: bool,
    destructive: bool,
    idempotent: bool,
) -> Json {
    crate::jobject! {
        "name" => name,
        "title" => title,
        "description" => description,
        "inputSchema" => schema,
        "annotations" => annotations(title, read_only, destructive, idempotent),
    }
}

fn common_write_fields() -> Vec<(String, Json)> {
    vec![
        ("title".to_string(), title_schema()),
        ("body".to_string(), body_schema()),
        ("entities".to_string(), entities_schema()),
        ("type".to_string(), type_schema()),
        ("scope".to_string(), scope_schema()),
    ]
}

fn without_key(schema: &Json, key: &str, description: Option<&str>) -> Json {
    let mut out = Vec::new();
    if let Json::Object(pairs) = schema {
        for (k, v) in pairs {
            if k == key || (description.is_some() && k == "description") {
                continue;
            }
            out.push((k.clone(), v.clone()));
        }
    }
    if let Some(description) = description {
        out.push(("description".to_string(), Json::Str(description.into())));
    }
    Json::Object(out)
}

pub fn tools() -> Json {
    let list = vec![
        tool(
            "memory_search",
            "搜索记忆",
            "按自然语言问题搜索已发布记忆，最多返回 5 条简短结果；需要正文时再读取。",
            object_schema(
                vec![("query".to_string(), crate::jobject! {
                    "type" => "string",
                    "minLength" => 1i64,
                    "maxLength" => MAX_QUERY_CHARS as i64,
                    "description" => "要回忆的问题或关键词",
                })],
                Some(vec!["query"]),
                Vec::new(),
            ),
            true,
            false,
            true,
        ),
        tool(
            "memory_get",
            "读取记忆",
            "按搜索结果中的编号读取完整记忆；单次读取 1 至 5 条。读取会更新最近使用日期。",
            object_schema(
                vec![("ids".to_string(), crate::jobject! {
                    "type" => "array",
                    "items" => id_schema(),
                    "minItems" => 1i64,
                    "maxItems" => GET_MAX_IDS as i64,
                    "uniqueItems" => true,
                    "description" => "要读取的记忆编号，保持需要的顺序",
                })],
                Some(vec!["ids"]),
                Vec::new(),
            ),
            false,
            false,
            false,
        ),
        tool(
            "memory_save",
            "保存记忆",
            "仅在用户以任何语言明确表达长期保存意图时保存正式记忆；不限定关键词，必须明确全局或当前项目范围。",
            object_schema(
                {
                    let mut fields = common_write_fields();
                    fields.push(("lock".to_string(), crate::jobject! {
                        "type" => "boolean",
                        "default" => false,
                        "description" => "用户要求牢牢记住且不得被自动削弱时设为 true",
                    }));
                    fields
                },
                Some(vec!["title", "scope"]),
                Vec::new(),
            ),
            false,
            false,
            true,
        ),
        tool(
            "memory_propose",
            "提议记忆",
            "仅当用户明确要求先存为候选时写入；不得由 AI 主动创建。候选不会进入默认检索与雷达。",
            object_schema(common_write_fields(), Some(vec!["title", "scope"]), Vec::new()),
            false,
            false,
            true,
        ),
        tool(
            "memory_publish",
            "发布候选",
            "把一条候选记忆发布为正式记忆；重复发布不会产生额外变更。",
            object_schema(vec![("id".to_string(), id_schema())], Some(vec!["id"]), Vec::new()),
            false,
            false,
            true,
        ),
        tool(
            "memory_reject",
            "丢弃候选",
            "丢弃候选记忆的正文并保留墓碑；重复丢弃不会产生额外变更。",
            object_schema(vec![("id".to_string(), id_schema())], Some(vec!["id"]), Vec::new()),
            false,
            true,
            true,
        ),
        tool(
            "memory_feedback",
            "记录反馈",
            "只有在使用记忆后获得明确对错证据时记录结果；不要凭感觉调用。",
            object_schema(
                vec![
                    ("id".to_string(), id_schema()),
                    ("outcome".to_string(), crate::jobject! {
                        "type" => "string",
                        "enum" => Json::Array(vec!["correct".into(), "incorrect".into()]),
                        "description" => "记忆内容被证实正确或不正确",
                    }),
                ],
                Some(vec!["id", "outcome"]),
                Vec::new(),
            ),
            false,
            true,
            false,
        ),
        tool(
            "memory_update",
            "更新记忆",
            "按字段修改正式记忆；未提供的字段保持不变，空正文或空实体数组表示明确清空。",
            object_schema(
                vec![
                    ("id".to_string(), id_schema()),
                    ("title".to_string(), title_schema()),
                    (
                        "body".to_string(),
                        without_key(&body_schema(), "default", Some("新正文；空字符串表示清空正文")),
                    ),
                    (
                        "entities".to_string(),
                        without_key(&entities_schema(), "default", Some("新实体列表；空数组表示清空实体")),
                    ),
                    ("type".to_string(), without_key(&type_schema(), "default", None)),
                ],
                Some(vec!["id"]),
                vec![("minProperties".to_string(), Json::Int(2))],
            ),
            false,
            true,
            true,
        ),
        tool(
            "memory_archive",
            "归档记忆",
            "归档正式记忆，使其退出默认检索与雷达但保留内容；重复归档不会产生额外变更。",
            object_schema(vec![("id".to_string(), id_schema())], Some(vec!["id"]), Vec::new()),
            false,
            true,
            true,
        ),
        tool(
            "memory_delete",
            "删除记忆",
            "把记忆改成不含正文的墓碑；必须明确传入 confirm=true。",
            object_schema(
                vec![
                    ("id".to_string(), id_schema()),
                    ("confirm".to_string(), crate::jobject! {
                        "type" => "boolean",
                        "const" => true,
                        "description" => "只有用户已明确确认删除时才能为 true",
                    }),
                ],
                Some(vec!["id", "confirm"]),
                Vec::new(),
            ),
            false,
            true,
            true,
        ),
        tool(
            "memory_audit",
            "检查记忆",
            "检查待审候选、长期未使用记忆和可能冲突，返回可读摘要，不修改数据。",
            object_schema(Vec::new(), None, Vec::new()),
            true,
            false,
            true,
        ),
    ];
    Json::Array(list)
}

pub const TOOL_NAMES: [&str; 11] = [
    "memory_search",
    "memory_get",
    "memory_save",
    "memory_propose",
    "memory_publish",
    "memory_reject",
    "memory_feedback",
    "memory_update",
    "memory_archive",
    "memory_delete",
    "memory_audit",
];

fn tool_input(message: impl Into<String>) -> Error {
    Error::Core(format!("\u{1}{}", message.into()))
}

fn check_keys(args: &Json, allowed: &[&str], required: &[&str]) -> std::result::Result<(), Error> {
    let keys = args.keys();
    let mut extra: Vec<&str> = keys
        .iter()
        .filter(|key| !allowed.contains(key))
        .copied()
        .collect();
    extra.sort_unstable();
    if !extra.is_empty() {
        return Err(tool_input(format!(
            "不支持参数 {}；请移除后重试",
            extra.join(", ")
        )));
    }
    let mut missing: Vec<&str> = required
        .iter()
        .filter(|key| !keys.contains(key))
        .copied()
        .collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        return Err(tool_input(format!(
            "缺少必填参数 {}；请补充后重试",
            missing.join(", ")
        )));
    }
    Ok(())
}

fn positive_id(value: Option<&Json>, label: &str) -> Result<i64> {
    let Some(value) = value else {
        return Err(tool_input(format!(
            "{label} 必须是大于等于 1 的整数记忆编号"
        )));
    };
    let id = match value {
        Json::Int(v) => *v,
        Json::Float(v) if v.fract() == 0.0 && *v > MAX_PUBLIC_ID as f64 => {
            return Err(tool_input(format!("{label} 超过安全上限 {MAX_PUBLIC_ID}")));
        }
        _ => {
            return Err(tool_input(format!(
                "{label} 必须是大于等于 1 的整数记忆编号"
            )))
        }
    };
    if id < 1 {
        return Err(tool_input(format!(
            "{label} 必须是大于等于 1 的整数记忆编号"
        )));
    }
    if id > MAX_PUBLIC_ID {
        return Err(tool_input(format!("{label} 超过安全上限 {MAX_PUBLIC_ID}")));
    }
    Ok(id)
}

fn required_string(value: Option<&Json>, label: &str, maximum: Option<usize>) -> Result<String> {
    let Some(text) = value.and_then(Json::as_str) else {
        return Err(tool_input(format!("{label} 必须是字符串")));
    };
    let normalized = text.trim();
    if normalized.is_empty() {
        return Err(tool_input(format!("{label} 不能为空")));
    }
    if maximum.is_some_and(|limit| py_len(normalized) > limit) {
        return Err(tool_input(format!(
            "{label} 最多 {} 个字符",
            maximum.unwrap()
        )));
    }
    Ok(normalized.to_string())
}

struct WriteArgs {
    title: String,
    body: String,
    entities: Vec<String>,
    card_type: String,
    scope: String,
    lock: bool,
}

fn common_write_args(args: &Json, allow_lock: bool) -> Result<WriteArgs> {
    let mut allowed = vec!["title", "body", "entities", "type", "scope"];
    if allow_lock {
        allowed.push("lock");
    }
    check_keys(args, &allowed, &["title", "scope"])?;
    let (title, body_lines, entities, card_type) = crate::mem::normalize_structured_content(
        args.get("title").and_then(Json::as_str).unwrap_or(""),
        args.get("body").and_then(Json::as_str).unwrap_or(""),
        &args
            .get("entities")
            .and_then(Json::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default(),
        args.get("type").and_then(Json::as_str).unwrap_or("fact"),
    )
    .map_err(|err| match err {
        Error::Core(message) => tool_input(message),
        other => other,
    })?;
    let scope = args.get("scope").and_then(Json::as_str).unwrap_or("");
    if scope != "global" && scope != "project" {
        return Err(tool_input("scope 只能是 global 或 project"));
    }
    let lock = if allow_lock {
        match args.get("lock") {
            None => false,
            Some(Json::Bool(value)) => *value,
            _ => return Err(tool_input("lock 必须是布尔值")),
        }
    } else {
        false
    };
    Ok(WriteArgs {
        title,
        body: body_lines.join("\n"),
        entities,
        card_type,
        scope: scope.to_string(),
        lock,
    })
}

enum Validated {
    Search {
        query: String,
    },
    Get {
        ids: Vec<i64>,
    },
    Write(WriteArgs),
    Id {
        id: i64,
    },
    Delete {
        id: i64,
    },
    Feedback {
        id: i64,
        outcome: String,
    },
    Update {
        id: i64,
        title: Option<String>,
        body: Option<String>,
        entities: Option<Vec<String>>,
        card_type: Option<String>,
    },
    Audit,
}

fn validate_tool_arguments(name: &str, raw_args: Option<&Json>) -> Result<Validated> {
    let args = match raw_args {
        Some(args) if args.is_object() => args.clone(),
        None => Json::Object(Vec::new()),
        _ => return Err(tool_input("工具参数必须是对象，请按工具说明重新调用")),
    };
    match name {
        "memory_search" => {
            check_keys(&args, &["query"], &["query"])?;
            let query = required_string(args.get("query"), "query", Some(MAX_QUERY_CHARS))?;
            Ok(Validated::Search { query })
        }
        "memory_get" => {
            check_keys(&args, &["ids"], &["ids"])?;
            let Some(items) = args.get("ids").and_then(Json::as_array) else {
                return Err(tool_input(format!(
                    "ids 必须包含 1 至 {GET_MAX_IDS} 个记忆编号"
                )));
            };
            if items.is_empty() || items.len() > GET_MAX_IDS {
                return Err(tool_input(format!(
                    "ids 必须包含 1 至 {GET_MAX_IDS} 个记忆编号"
                )));
            }
            let mut normalized = Vec::new();
            for item in items {
                normalized.push(positive_id(Some(item), "ids 中的编号")?);
            }
            let mut unique = normalized.clone();
            unique.sort_unstable();
            unique.dedup();
            if unique.len() != normalized.len() {
                return Err(tool_input("ids 不能包含重复编号"));
            }
            Ok(Validated::Get { ids: normalized })
        }
        "memory_save" => Ok(Validated::Write(common_write_args(&args, true)?)),
        "memory_propose" => Ok(Validated::Write(common_write_args(&args, false)?)),
        "memory_publish" | "memory_reject" | "memory_archive" => {
            check_keys(&args, &["id"], &["id"])?;
            Ok(Validated::Id {
                id: positive_id(args.get("id"), "id")?,
            })
        }
        "memory_delete" => {
            check_keys(&args, &["id", "confirm"], &["id", "confirm"])?;
            if args.get("confirm").and_then(Json::as_bool) != Some(true) {
                return Err(tool_input(
                    "删除记忆需要用户明确确认；确认后请传入 confirm=true",
                ));
            }
            Ok(Validated::Delete {
                id: positive_id(args.get("id"), "id")?,
            })
        }
        "memory_feedback" => {
            check_keys(&args, &["id", "outcome"], &["id", "outcome"])?;
            let outcome = args.get("outcome").and_then(Json::as_str).unwrap_or("");
            if outcome != "correct" && outcome != "incorrect" {
                return Err(tool_input("outcome 只能是 correct 或 incorrect"));
            }
            Ok(Validated::Feedback {
                id: positive_id(args.get("id"), "id")?,
                outcome: outcome.to_string(),
            })
        }
        "memory_update" => {
            check_keys(&args, &["id", "title", "body", "entities", "type"], &["id"])?;
            if args.keys().len() < 2 {
                return Err(tool_input(
                    "除 id 外至少提供 title、body、entities 或 type 中的一项",
                ));
            }
            let id = positive_id(args.get("id"), "id")?;
            let (norm_title, norm_body, norm_entities, norm_type) =
                crate::mem::normalize_structured_content(
                    args.get("title")
                        .and_then(Json::as_str)
                        .unwrap_or("临时标题"),
                    args.get("body").and_then(Json::as_str).unwrap_or(""),
                    &args
                        .get("entities")
                        .and_then(Json::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(str::to_string))
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_default(),
                    args.get("type").and_then(Json::as_str).unwrap_or("fact"),
                )
                .map_err(|err| match err {
                    Error::Core(message) => tool_input(message),
                    other => other,
                })?;
            Ok(Validated::Update {
                id,
                title: if args.get("title").is_some() {
                    Some(norm_title)
                } else {
                    None
                },
                body: if args.get("body").is_some() {
                    Some(norm_body.join("\n"))
                } else {
                    None
                },
                entities: if args.get("entities").is_some() {
                    Some(norm_entities)
                } else {
                    None
                },
                card_type: if args.get("type").is_some() {
                    Some(norm_type)
                } else {
                    None
                },
            })
        }
        "memory_audit" => {
            check_keys(&args, &[], &[])?;
            Ok(Validated::Audit)
        }
        _ => Err(tool_input(format!("未知工具 {name}"))),
    }
}

fn ok_text(payload: &str) -> Json {
    crate::jobject! {
        "content" => Json::Array(vec![crate::jobject! {
            "type" => "text",
            "text" => payload,
        }]),
    }
}

fn error_text(message: &str) -> Json {
    let mut payload = ok_text(&format!("无法完成：{message}"));
    if let Json::Object(ref mut pairs) = payload {
        pairs.push(("isError".to_string(), Json::Bool(true)));
    }
    payload
}

/// _human_error: @N → 记忆 N (no word boundary), then status word replacements.
fn human_error(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let bytes = message.as_bytes();
    let mut pos = 0usize;
    while pos < bytes.len() {
        if bytes[pos] == b'@' {
            let start = pos + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                out.push_str("记忆 ");
                out.push_str(&message[start..end]);
                pos = end;
                continue;
            }
        }
        let ch_len = message[pos..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        out.push_str(&message[pos..pos + ch_len]);
        pos += ch_len;
    }
    out.replace("candidate", "候选")
        .replace("published", "正式")
        .replace("archived", "已归档")
        .replace("tombstone", "已删除")
}

fn human_audit(report: &Json) -> String {
    let stale = report.get("stale").and_then(Json::as_array).unwrap_or(&[]);
    let unused = report
        .get("unused_90d")
        .and_then(Json::as_array)
        .unwrap_or(&[]);
    let candidates = report
        .get("candidates")
        .and_then(Json::as_array)
        .unwrap_or(&[]);
    let conflicts = report
        .get("possible_conflicts")
        .and_then(Json::as_array)
        .unwrap_or(&[]);
    let total = stale.len() + unused.len() + candidates.len() + conflicts.len();
    if total == 0 {
        return "检查完成：没有发现需要处理的记忆。".into();
    }
    let mut lines = vec![format!("检查完成：发现 {total} 项需要关注。")];
    let title_of = |item: &Json| {
        human_display_title(item.get("title").and_then(Json::as_str).unwrap_or(""), 160)
    };
    if !candidates.is_empty() {
        lines.push("待审候选：".into());
        for item in candidates {
            lines.push(format!(
                "- 记忆 {}：{}",
                item.get("id").and_then(Json::as_i64).unwrap_or(0),
                title_of(item)
            ));
        }
    }
    if !stale.is_empty() {
        lines.push("可能已经失去价值：".into());
        for item in stale {
            lines.push(format!(
                "- 记忆 {}：{}",
                item.get("id").and_then(Json::as_i64).unwrap_or(0),
                title_of(item)
            ));
        }
    }
    if !unused.is_empty() {
        lines.push("超过 90 天未使用：".into());
        for item in unused {
            lines.push(format!(
                "- 记忆 {}：{}（{} 天）",
                item.get("id").and_then(Json::as_i64).unwrap_or(0),
                title_of(item),
                item.get("days").and_then(Json::as_i64).unwrap_or(0)
            ));
        }
    }
    if !conflicts.is_empty() {
        lines.push("可能存在冲突：".into());
        for item in conflicts {
            let anchor = item.get("anchor").and_then(Json::as_str).unwrap_or("");
            let ids: Vec<String> = item
                .get("ids")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|id| id.as_i64().map(|v| v.to_string()))
                .collect();
            lines.push(format!(
                "- “{}”关联了记忆 {}",
                humanize_memory_text(anchor),
                ids.join("、")
            ));
        }
    }
    lines.join("\n")
}

fn ensure_visible(layout: &Layout, ids: &[i64], project: &str) -> Result<()> {
    let metas = cache::get_meta(layout, ids)?;
    for memory_id in ids {
        let meta = metas.iter().find(|meta| meta.id == *memory_id);
        match meta {
            Some(meta) if crate::radar::scope_visible(&meta.scope, project) => {}
            _ => {
                return Err(Error::core(format!("当前范围内不存在记忆 {memory_id}")));
            }
        }
    }
    Ok(())
}

fn call_tool(layout: &Layout, name: &str, validated: &Validated, project: &str) -> Result<Json> {
    match validated {
        Validated::Search { query } => {
            let cfg = config::load_config(&layout.home);
            let rows = search::search(layout, query, "published", 5, project, Some(&cfg))?;
            if rows.is_empty() {
                return Ok(ok_text("没有找到相关记忆。"));
            }
            let mut lines = vec![format!("找到 {} 条相关记忆：", rows.len())];
            let search_cfg = config::section(&cfg, "search");
            for (position, row) in rows.iter().enumerate() {
                lines.push(format!(
                    "- {}",
                    human_search_line(row, position, search_cfg)
                ));
            }
            Ok(ok_text(&lines.join("\n")))
        }
        Validated::Get { ids } => {
            ensure_visible(layout, ids, project)?;
            let cards = lifecycle::get_cards(layout, ids, Some(project))?;
            if cards.is_empty() {
                return Ok(error_text("这些编号对应的记忆不存在，请先搜索后重试"));
            }
            let mut parts = Vec::new();
            for card in &cards {
                let suffix = if card.truncated {
                    "\n（正文过长，已安全截断。）"
                } else {
                    ""
                };
                parts.push(format!(
                    "记忆 {}：{}{}",
                    card.id,
                    humanize_memory_text(&card.text),
                    suffix
                ));
            }
            Ok(ok_text(&parts.join("\n\n")))
        }
        Validated::Write(args) => {
            let published = name == "memory_save";
            let status = if published { "published" } else { "candidate" };
            let source = if published { "user" } else { "self:agent" };
            let card = lifecycle::write_structured_card(
                layout,
                &args.title,
                &args.body,
                &args.entities,
                &args.card_type,
                &args.scope,
                project,
                status,
                source,
                args.lock,
            )?;
            if card.deduplicated {
                if !card.unchanged {
                    return Ok(ok_text(&format!(
                        "已复用现有内容并保存：记忆 {}：{}",
                        card.id,
                        humanize_memory_text(&card.title)
                    )));
                }
                return Ok(ok_text(&format!(
                    "相同内容已经存在，无需重复写入：记忆 {}：{}",
                    card.id,
                    humanize_memory_text(&card.title)
                )));
            }
            let action = if published {
                "已保存"
            } else {
                "已加入待审候选"
            };
            Ok(ok_text(&format!(
                "{action}：记忆 {}：{}",
                card.id,
                humanize_memory_text(&card.title)
            )))
        }
        Validated::Id { id } => {
            let card = match name {
                "memory_publish" => lifecycle::publish(layout, *id, Some(project))?,
                "memory_reject" => lifecycle::reject(layout, *id, Some(project))?,
                "memory_archive" => lifecycle::archive_card(layout, *id, Some(project))?,
                _ => unreachable!(),
            };
            let action = match (name, card.unchanged) {
                ("memory_publish", true) => "已经是正式记忆",
                ("memory_publish", false) => "已发布候选",
                ("memory_reject", true) => "已经被丢弃",
                ("memory_reject", false) => "已丢弃候选",
                ("memory_archive", true) => "已经归档",
                ("memory_archive", false) => "已归档",
                _ => unreachable!(),
            };
            if name == "memory_reject" {
                return Ok(ok_text(&format!("{action}：记忆 {id}")));
            }
            Ok(ok_text(&format!(
                "{action}：记忆 {}：{}",
                card.id,
                humanize_memory_text(&card.title)
            )))
        }
        Validated::Delete { id } => {
            let card = lifecycle::tombstone_card(layout, *id, true, Some(project))?;
            let action = if card.unchanged {
                "已经删除"
            } else {
                "已删除"
            };
            Ok(ok_text(&format!("{action}：记忆 {id}")))
        }
        Validated::Feedback { id, outcome } => {
            let signal = if outcome == "correct" { "+" } else { "-" };
            lifecycle::feedback(layout, *id, signal, Some(project))?;
            let conclusion = if signal == "+" { "正确" } else { "不正确" };
            Ok(ok_text(&format!("已记录：记忆 {id} 的内容{conclusion}。")))
        }
        Validated::Update {
            id,
            title,
            body,
            entities,
            card_type,
        } => {
            let card = lifecycle::update_card_fields(
                layout,
                *id,
                title.as_deref(),
                body.as_deref(),
                entities.as_deref(),
                card_type.as_deref(),
                Some(project),
            )?;
            let action = if card.unchanged {
                "内容没有变化"
            } else {
                "已更新"
            };
            Ok(ok_text(&format!(
                "{action}：记忆 {}：{}",
                card.id,
                humanize_memory_text(&card.title)
            )))
        }
        Validated::Audit => {
            let report = lifecycle::audit(layout, Some(project))?;
            Ok(ok_text(&human_audit(&report)))
        }
    }
}

// --- project context resolution ---

fn file_root(uri: &Json) -> Option<String> {
    let uri = uri.as_str()?;
    let rest = uri.strip_prefix("file://")?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if !authority.is_empty() && authority != "localhost" {
        return None;
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(path)?;
    #[cfg(windows)]
    let decoded = if decoded.as_bytes().get(0) == Some(&b'/')
        && decoded.as_bytes().get(2) == Some(&b':')
        && decoded
            .as_bytes()
            .get(1)
            .is_some_and(u8::is_ascii_alphabetic)
    {
        decoded[1..].to_string()
    } else {
        decoded
    };
    Some(decoded)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    let hex = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    while pos < bytes.len() {
        if bytes[pos] == b'%' {
            if pos + 2 >= bytes.len() {
                return None;
            }
            let (Some(high), Some(low)) = (hex(bytes[pos + 1]), hex(bytes[pos + 2])) else {
                return None;
            };
            out.push(high * 16 + low);
            pos += 3;
            continue;
        }
        out.push(bytes[pos]);
        pos += 1;
    }
    String::from_utf8(out).ok()
}

fn canonical_root(value: Option<&str>, authoritative: bool, layout: &Layout) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    let expanded = crate::paths::expand_user(value);
    if !expanded.is_absolute() {
        return None;
    }
    let path = std::fs::canonicalize(&expanded).ok()?;
    if !path.is_dir() {
        return None;
    }
    let data_home = crate::paths::resolve_lenient(&layout.home);
    if path == data_home || path.starts_with(&data_home) {
        return None;
    }
    project_directory(Some(path.to_str()?), authoritative, layout)
}

fn resolve_project_context(
    root_uris: &[String],
    cwd: Option<&str>,
    layout: &Layout,
) -> (String, &'static str) {
    let mut roots: Vec<PathBuf> = Vec::new();
    for uri in root_uris {
        let path_value = file_root(&Json::Str(uri.clone()));
        let root = path_value.and_then(|path| canonical_root(Some(&path), true, layout));
        if let Some(root) = root {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    if roots.len() == 1 {
        return (project_id(roots[0].to_str(), layout), "roots");
    }
    let cwd_value = cwd.map(str::to_string).or_else(|| {
        std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
    });
    let cwd_root = cwd_value.and_then(|cwd| canonical_root(Some(&cwd), false, layout));
    if let Some(root) = cwd_root {
        return (project_id(root.to_str(), layout), "cwd");
    }
    ("global".into(), "unknown")
}

// --- server state and main loop ---

struct ServerState {
    protocol_version: String,
    initialized: bool,
    client_name: String,
    client_version: String,
    roots_supported: bool,
    root_uris: Vec<String>,
    roots_request_id: String,
    roots_request_number: i64,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            protocol_version: LATEST_PROTOCOL_VERSION.into(),
            initialized: false,
            client_name: "unknown".into(),
            client_version: "unknown".into(),
            roots_supported: false,
            root_uris: Vec::new(),
            roots_request_id: String::new(),
            roots_request_number: 0,
        }
    }

    fn reset(&mut self, params: &Json) {
        let requested = params.get("protocolVersion").and_then(Json::as_str);
        self.protocol_version = match requested {
            Some(version) if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => version.into(),
            _ => LATEST_PROTOCOL_VERSION.into(),
        };
        let client = params.get("clientInfo").filter(|info| info.is_object());
        self.client_name = client
            .and_then(|info| info.get("name").and_then(Json::as_str))
            .unwrap_or("unknown")
            .chars()
            .take(80)
            .collect();
        self.client_version = client
            .and_then(|info| info.get("version").and_then(Json::as_str))
            .unwrap_or("unknown")
            .chars()
            .take(40)
            .collect();
        self.roots_supported = params
            .get("capabilities")
            .and_then(|caps| caps.get("roots"))
            .is_some_and(|roots| roots.is_object());
        self.root_uris = Vec::new();
        self.roots_request_id = String::new();
        self.initialized = true;
    }

    fn project(&self, layout: &Layout) -> (String, &'static str) {
        if self.roots_supported && !self.roots_request_id.is_empty() {
            return ("global".into(), "unknown");
        }
        resolve_project_context(&self.root_uris, None, layout)
    }
}

struct Server<'a> {
    layout: &'a Layout,
    state: ServerState,
    stdout: std::io::Stdout,
}

fn json_size(value: &Json) -> i64 {
    value.dumps_canonical().len() as i64
}

fn send(stdout: &mut std::io::Stdout, message: &Json) {
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{}", message.dumps_canonical());
    let _ = lock.flush();
}

fn send_result(stdout: &mut std::io::Stdout, request_id: &Json, payload: Json) {
    send(
        stdout,
        &crate::jobject! {
            "jsonrpc" => "2.0",
            "id" => request_id.clone(),
            "result" => payload,
        },
    );
}

fn send_error(stdout: &mut std::io::Stdout, request_id: &Json, code: i64, message: &str) {
    send(
        stdout,
        &crate::jobject! {
            "jsonrpc" => "2.0",
            "id" => request_id.clone(),
            "error" => crate::jobject! {
                "code" => code,
                "message" => message,
            },
        },
    );
}

fn sanitize_request_id(value: Option<&Json>) -> Json {
    match value {
        Some(Json::Int(v)) => Json::Int(*v),
        Some(Json::Float(v)) => Json::Float(*v),
        Some(Json::Str(text)) => {
            if text.len() <= 64
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            {
                Json::Str(text.clone())
            } else {
                Json::Null
            }
        }
        _ => Json::Null,
    }
}

impl<'a> Server<'a> {
    fn log(&self, event: &str, fields: Vec<(String, Json)>) {
        // Logging must never initialize a missing data root by itself.
        if !self.layout.home.is_dir() {
            return;
        }
        let logs = self.layout.logs();
        if crate::durable_fs::create_dir_all_private(&logs).is_err() {
            return;
        }
        let _ = crate::durable_fs::chmod_private(&logs, true);
        let path = logs.join("mcp.log");
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 500_000 {
                let _ = crate::durable_fs::atomic_write(&path, "");
            }
        }
        let mut record: Vec<(String, Json)> = vec![
            (
                "time".to_string(),
                Json::Str(crate::clock::clock().isoformat_seconds()),
            ),
            ("request_id".to_string(), Json::Null),
            (
                "client_name".to_string(),
                Json::Str(self.state.client_name.clone()),
            ),
            (
                "client_version".to_string(),
                Json::Str(self.state.client_version.clone()),
            ),
            (
                "protocol_version".to_string(),
                Json::Str(self.state.protocol_version.clone()),
            ),
            ("tool".to_string(), Json::Str(event.to_string())),
            ("duration_ms".to_string(), Json::Null),
            ("ok".to_string(), Json::Null),
            ("args_bytes".to_string(), Json::Null),
            ("result_bytes".to_string(), Json::Null),
        ];
        for (key, value) in fields {
            match key.as_str() {
                "request_id" => record[1].1 = value,
                "tool" => {
                    record[5].1 =
                        Json::Str(value.as_str().unwrap_or(event).chars().take(80).collect())
                }
                "duration_ms" => record[6].1 = value,
                "ok" => record[7].1 = value,
                "args_bytes" => record[8].1 = value,
                "result_bytes" => record[9].1 = value,
                "correlation" => record.push((
                    "correlation".into(),
                    Json::Str(value.as_str().unwrap_or("").chars().take(32).collect()),
                )),
                "error_type" => record.push((
                    "error_type".into(),
                    Json::Str(value.as_str().unwrap_or("").chars().take(80).collect()),
                )),
                _ => {}
            }
        }
        let line = Json::Object(record).dumps_canonical();
        if let Ok(mut file) = crate::durable_fs::open_private_append(&path) {
            let _ = writeln!(file, "{line}");
        }
    }

    fn request_roots(&mut self) {
        if !self.state.roots_supported || !self.state.roots_request_id.is_empty() {
            return;
        }
        self.state.roots_request_number += 1;
        self.state.roots_request_id =
            format!("engramark-roots-{}", self.state.roots_request_number);
        let id = self.state.roots_request_id.clone();
        send(
            &mut self.stdout,
            &crate::jobject! {
                "jsonrpc" => "2.0",
                "id" => id,
                "method" => "roots/list",
            },
        );
    }

    fn handle_roots_response(&mut self, message: &Json) -> bool {
        if self.state.roots_request_id.is_empty()
            || message.get("id").and_then(Json::as_str)
                != Some(self.state.roots_request_id.as_str())
        {
            return false;
        }
        let mut uris = Vec::new();
        if let Some(roots) = message
            .get("result")
            .and_then(|result| result.get("roots"))
            .and_then(Json::as_array)
        {
            for root in roots {
                if let Some(uri) = root.get("uri").and_then(Json::as_str) {
                    uris.push(uri.to_string());
                }
            }
        }
        self.state.root_uris = uris;
        self.state.roots_request_id = String::new();
        let count = self.state.root_uris.len() as i64;
        let (project, _) = self.state.project(self.layout);
        self.log(
            "roots",
            vec![
                ("tool".into(), Json::Str("roots".into())),
                ("args_bytes".into(), Json::Int(count)),
                ("ok".into(), Json::Bool(project != "global")),
            ],
        );
        true
    }

    fn handle_request(&mut self, request: &Json, input_bytes: i64) {
        let method = request.get("method").and_then(Json::as_str).unwrap_or("");
        let request_id = request.get("id").cloned().unwrap_or(Json::Null);
        let params = request
            .get("params")
            .cloned()
            .unwrap_or(Json::Object(Vec::new()));
        let started = Instant::now();
        match method {
            "initialize" => {
                if !params.is_object() {
                    send_error(
                        &mut self.stdout,
                        &request_id,
                        -32602,
                        "initialize 参数必须是对象",
                    );
                    return;
                }
                self.state.reset(&params);
                let payload = crate::jobject! {
                    "protocolVersion" => self.state.protocol_version.clone(),
                    "capabilities" => crate::jobject! {
                        "tools" => crate::jobject! { "listChanged" => false },
                    },
                    "serverInfo" => crate::jobject! {
                        "name" => SERVER_NAME,
                        "version" => VERSION,
                    },
                    "instructions" => SERVER_INSTRUCTIONS,
                };
                send_result(&mut self.stdout, &request_id, payload.clone());
                self.log(
                    "initialize",
                    vec![
                        ("request_id".into(), sanitize_request_id(Some(&request_id))),
                        ("args_bytes".into(), Json::Int(input_bytes)),
                        ("result_bytes".into(), Json::Int(json_size(&payload))),
                        ("ok".into(), Json::Bool(true)),
                    ],
                );
            }
            _ if !self.state.initialized => {
                if !request_id.is_null() {
                    send_error(&mut self.stdout, &request_id, -32002, "服务尚未初始化");
                }
            }
            "notifications/initialized" | "notifications/roots/list_changed" => {
                self.request_roots();
            }
            "notifications/cancelled" => {}
            "ping" => {
                send_result(&mut self.stdout, &request_id, Json::Object(Vec::new()));
            }
            "tools/list" => {
                let payload = crate::jobject! { "tools" => tools() };
                send_result(&mut self.stdout, &request_id, payload.clone());
                self.log(
                    "request",
                    vec![
                        ("request_id".into(), sanitize_request_id(Some(&request_id))),
                        ("tool".into(), Json::Str("tools/list".into())),
                        ("args_bytes".into(), Json::Int(input_bytes)),
                        ("result_bytes".into(), Json::Int(json_size(&payload))),
                        ("ok".into(), Json::Bool(true)),
                    ],
                );
            }
            "tools/call" => self.handle_tool_call(&request_id, &params, input_bytes, started),
            "resources/list" => {
                send_result(
                    &mut self.stdout,
                    &request_id,
                    crate::jobject! {
                        "resources" => Json::Array(Vec::new()),
                    },
                );
            }
            "resources/templates/list" => {
                send_result(
                    &mut self.stdout,
                    &request_id,
                    crate::jobject! {
                        "resourceTemplates" => Json::Array(Vec::new()),
                    },
                );
            }
            "prompts/list" => {
                send_result(
                    &mut self.stdout,
                    &request_id,
                    crate::jobject! {
                        "prompts" => Json::Array(Vec::new()),
                    },
                );
            }
            _ => {
                if !request_id.is_null() {
                    send_error(
                        &mut self.stdout,
                        &request_id,
                        -32601,
                        &format!("不支持的方法：{method}"),
                    );
                }
            }
        }
    }

    fn handle_tool_call(
        &mut self,
        request_id: &Json,
        params: &Json,
        input_bytes: i64,
        started: Instant,
    ) {
        let _ = input_bytes;
        if !params.is_object() || params.get("name").and_then(Json::as_str).is_none() {
            send_error(&mut self.stdout, request_id, -32602, "tools/call 参数无效");
            return;
        }
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        if !TOOL_NAMES.contains(&name.as_str()) {
            send_error(
                &mut self.stdout,
                request_id,
                -32602,
                &format!("未知工具：{name}"),
            );
            return;
        }
        let raw_args = params
            .get("arguments")
            .cloned()
            .unwrap_or(Json::Object(Vec::new()));
        let args_bytes = json_size(&raw_args);
        let duration =
            || Json::Float(((started.elapsed().as_secs_f64() * 1000.0) * 100.0).round() / 100.0);
        let validated = validate_tool_arguments(&name, Some(&raw_args));
        let (project, _source) = self.state.project(self.layout);
        match validated.and_then(|validated| call_tool(self.layout, &name, &validated, &project)) {
            Ok(payload) => {
                send_result(&mut self.stdout, request_id, payload.clone());
                let tool_ok = payload.get("isError").and_then(Json::as_bool) != Some(true);
                self.log(
                    "tool_call",
                    vec![
                        ("request_id".into(), sanitize_request_id(Some(request_id))),
                        ("tool".into(), Json::Str(name.clone())),
                        ("duration_ms".into(), duration()),
                        ("ok".into(), Json::Bool(tool_ok)),
                        ("args_bytes".into(), Json::Int(args_bytes)),
                        ("result_bytes".into(), Json::Int(json_size(&payload))),
                    ],
                );
            }
            Err(err) => {
                let (payload_result, error_kind) = self.tool_error_payload(err);
                match payload_result {
                    Ok(payload) => {
                        send_result(&mut self.stdout, request_id, payload.clone());
                        self.log(
                            "tool_call",
                            vec![
                                ("request_id".into(), sanitize_request_id(Some(request_id))),
                                ("tool".into(), Json::Str(name.clone())),
                                ("duration_ms".into(), duration()),
                                ("ok".into(), Json::Bool(false)),
                                ("error_type".into(), Json::Str(error_kind.to_string())),
                                ("args_bytes".into(), Json::Int(args_bytes)),
                                ("result_bytes".into(), Json::Int(json_size(&payload))),
                            ],
                        );
                    }
                    Err(correlation) => {
                        self.log(
                            "internal_error",
                            vec![
                                ("request_id".into(), sanitize_request_id(Some(request_id))),
                                ("tool".into(), Json::Str(name.clone())),
                                ("error_type".into(), Json::Str(error_kind.to_string())),
                                ("correlation".into(), Json::Str(correlation.clone())),
                            ],
                        );
                        send_error(
                            &mut self.stdout,
                            request_id,
                            -32603,
                            &format!("服务内部错误，请稍后重试（编号 {correlation}）"),
                        );
                    }
                }
            }
        }
    }

    fn tool_error_payload(
        &mut self,
        err: Error,
    ) -> (std::result::Result<Json, String>, &'static str) {
        match err {
            Error::CacheUnavailable(_) => (
                Ok(error_text(
                    "记忆索引暂时不可用，原始记忆没有丢失。请稍后重试；若持续出现，请运行诊断。",
                )),
                "CacheUnavailable",
            ),
            Error::Core(message) if message.starts_with('\u{1}') => (
                Ok(error_text(&human_error(&message[1..]))),
                "ToolInputError",
            ),
            Error::Core(message) if public_core_error(&message) => {
                (Ok(error_text(&human_error(&message))), "CoreError")
            }
            other => (
                Err(crate::clock::clock().uuid4().replace('-', "")[..12].to_string()),
                error_type_name(&other),
            ),
        }
    }
}

fn error_type_name(err: &Error) -> &'static str {
    match err {
        Error::Core(_) => "CoreError",
        Error::CacheUnavailable(_) => "CacheUnavailable",
        Error::LockTimeout(_) => "LockTimeout",
        Error::HookProtocol(_) => "HookProtocolError",
        Error::HookCandidateOverflow => "HookCandidateOverflow",
        Error::HookDeadlineExceeded => "HookDeadlineExceeded",
        Error::HookUnavailable(_) => "HookUnavailable",
    }
}

fn public_core_error(message: &str) -> bool {
    let patterns = [
        r"^(?:标题|正文|实体|每个实体|单个实体|类型|适用范围|scope=project|记忆内容)",
        r"^查询超过 \d+ 字符上限$",
        r"^查询超过时间预算",
        r"^单次最多取 ",
        r"^当前范围内不存在记忆 \d+$",
        r"^候选记忆 \d+ 不存在$",
        r"^记忆 \d+ (?:不存在|不是)",
        r"^卡片 @\d+ 不存在$",
        r"^@\d+ (?:不是|已被|已不再|今天已)",
        r"^删除已发布卡片必须显式确认$",
    ];
    patterns.iter().any(|pattern| {
        regex::Regex::new(pattern)
            .map(|re| re.is_match(message))
            .unwrap_or(false)
    })
}

fn valid_request(message: &Json) -> bool {
    message.is_object()
        && message.get("jsonrpc").and_then(Json::as_str) == Some("2.0")
        && message.get("method").and_then(Json::as_str).is_some()
}

/// Read one NDJSON frame with a hard cap; oversized frames are drained to the
/// next newline without unbounded allocation.
enum Frame {
    Message(Vec<u8>),
    Oversized,
    Eof,
}

fn read_frame(reader: &mut impl BufRead) -> Frame {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let available = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(_) => return Frame::Eof,
        };
        if available.is_empty() {
            return if buf.is_empty() {
                Frame::Eof
            } else {
                Frame::Message(buf)
            };
        }
        let take = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|pos| pos + 1)
            .unwrap_or(available.len());
        if buf.len() + take > MAX_FRAME_BYTES {
            reader.consume(take);
            // Drain to the next newline, bounded.
            loop {
                let (skip, found) = {
                    let chunk = match reader.fill_buf() {
                        Ok(chunk) => chunk,
                        Err(_) => return Frame::Oversized,
                    };
                    if chunk.is_empty() {
                        return Frame::Oversized;
                    }
                    match chunk.iter().position(|b| *b == b'\n') {
                        Some(pos) => (pos + 1, true),
                        None => (chunk.len(), false),
                    }
                };
                reader.consume(skip);
                if found {
                    return Frame::Oversized;
                }
            }
        }
        buf.extend_from_slice(&available[..take]);
        let done = available[..take].contains(&b'\n');
        reader.consume(take);
        if done {
            return Frame::Message(buf);
        }
    }
}

pub fn main_loop(layout: &Layout) -> Result<()> {
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, stdin.lock());
    let mut server = Server {
        layout,
        state: ServerState::new(),
        stdout: std::io::stdout(),
    };
    server.log(
        "server_start",
        vec![("tool".into(), Json::Str(format!("server_start v{VERSION}")))],
    );
    loop {
        let frame = read_frame(&mut reader);
        let bytes = match frame {
            Frame::Eof => break,
            Frame::Oversized => {
                send_error(&mut server.stdout, &Json::Null, -32700, "JSON 帧超过上限");
                server.log(
                    "protocol_error",
                    vec![("error_type".into(), Json::Str("-32700".into()))],
                );
                continue;
            }
            Frame::Message(bytes) => bytes,
        };
        let input_bytes = bytes.len() as i64;
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(_) => {
                send_error(&mut server.stdout, &Json::Null, -32700, "JSON 解析失败");
                continue;
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        let message = match Json::parse(text) {
            Ok(message) => message,
            Err(_) => {
                send_error(&mut server.stdout, &Json::Null, -32700, "JSON 解析失败");
                server.log(
                    "protocol_error",
                    vec![("error_type".into(), Json::Str("-32700".into()))],
                );
                continue;
            }
        };
        if message.is_object() && server.handle_roots_response(&message) {
            continue;
        }
        if message.is_object()
            && message.get("jsonrpc").and_then(Json::as_str) == Some("2.0")
            && message.get("id").is_some()
            && (message.get("result").is_some() || message.get("error").is_some())
            && message.get("method").is_none()
        {
            server.log(
                "unexpected_response",
                vec![("request_id".into(), sanitize_request_id(message.get("id")))],
            );
            continue;
        }
        if !valid_request(&message) {
            let request_id = message.get("id").cloned().unwrap_or(Json::Null);
            send_error(
                &mut server.stdout,
                &request_id,
                -32600,
                "无效的 JSON-RPC 请求",
            );
            server.log(
                "protocol_error",
                vec![("error_type".into(), Json::Str("-32600".into()))],
            );
            continue;
        }
        server.handle_request(&message, input_bytes);
    }
    server.log("server_stop", Vec::new());
    Ok(())
}
