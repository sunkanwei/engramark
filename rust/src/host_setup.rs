//! Host wiring: surgical edits of Codex TOML/hooks.json/AGENTS.md and
//! OpenCode JSONC/AGENTS.md/plugin, with backups and failure rollback.
//! Non-target bytes, comments, trailing commas, indentation and property
//! order are preserved; whole-file re-serialization is never used.

use std::path::{Path, PathBuf};

use crate::json::Json;
use crate::paths::Layout;
use crate::{Error, Result, VERSION};

pub const AGENT_BEGIN: &str = "<!-- engramark-begin -->";
pub const AGENT_END: &str = "<!-- engramark-end -->";
pub const CODEX_MEMORY_BEGIN: &str = "# engramark-begin codex-memory";
pub const CODEX_MEMORY_END: &str = "# engramark-end codex-memory";
pub const CODEX_MCP_BEGIN: &str = "# engramark-begin codex-mcp";
pub const CODEX_MCP_END: &str = "# engramark-end codex-mcp";
pub const CODEX_PROJECT_BEGIN: &str = "# engramark-begin project-cwd";
pub const CODEX_PROJECT_END: &str = "# engramark-end project-cwd";

const AGENT_BLOCK_TEMPLATE: &str = include_str!("../../docs/agent_block.txt");
const HOOKS_TEMPLATE: &str = include_str!("../../adapters/codex/hooks.json");
const OPENCODE_PLUGIN: &str = include_str!("../../adapters/opencode/engramark.js");

fn setup_error(message: impl Into<String>) -> Error {
    Error::Core(format!("\u{2}{}", message.into()))
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "engramark.exe"
    } else {
        "engramark"
    }
}

fn binary_path(app_root: &Path) -> PathBuf {
    app_root.join("bin").join(binary_name())
}

pub struct HostSetupArgs {
    pub action: String,
    pub home: Option<String>,
    pub app_root: Option<String>,
    pub data_home: Option<String>,
    pub codex: String,
    pub opencode: String,
    pub project: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub path: PathBuf,
    pub content: Option<Vec<u8>>,
}

// --- JSONC tokenizer / parser (character-index spans, like the Python one) ---

#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: String,
    value: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Debug)]
struct Property {
    key: String,
    start: usize,
    end: usize,
    value: Node,
    comma_before: Option<Token>,
    comma_after: Option<Token>,
}

#[derive(Clone, Debug)]
struct Node {
    kind: String,
    start: usize,
    end: usize,
    close_start: usize,
    properties: Vec<Property>,
}

fn lex_jsonc(chars: &[char]) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let text: String = chars.iter().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let char = chars[index];
        if char.is_whitespace() {
            index += 1;
            continue;
        }
        if text[index..].starts_with("//") {
            let mut end = index + 2;
            while end < chars.len() && chars[end] != '\n' {
                end += 1;
            }
            index = if end < chars.len() {
                end + 1
            } else {
                chars.len()
            };
            continue;
        }
        if text[index..].starts_with("/*") {
            let mut end = index + 2;
            while end + 1 < chars.len() && !(chars[end] == '*' && chars[end + 1] == '/') {
                end += 1;
            }
            if end + 1 >= chars.len() {
                return Err(setup_error("OpenCode 配置包含未结束的注释"));
            }
            index = end + 2;
            continue;
        }
        if "{}[]:,".contains(char) {
            tokens.push(Token {
                kind: char.to_string(),
                value: char.to_string(),
                start: index,
                end: index + 1,
            });
            index += 1;
            continue;
        }
        if char == '"' {
            let start = index;
            index += 1;
            let mut escaped = false;
            let mut closed = false;
            while index < chars.len() {
                let current = chars[index];
                index += 1;
                if escaped {
                    escaped = false;
                } else if current == '\\' {
                    escaped = true;
                } else if current == '"' {
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(setup_error("OpenCode 配置包含未结束的字符串"));
            }
            let raw: String = chars[start..index].iter().collect();
            let value = Json::parse(&raw)
                .map_err(|err| setup_error(format!("OpenCode 配置字符串无效：{err}")))?;
            let Some(value) = value.as_str().map(str::to_string) else {
                return Err(setup_error("OpenCode 配置字符串无效：不是字符串"));
            };
            tokens.push(Token {
                kind: "string".into(),
                value,
                start,
                end: index,
            });
            continue;
        }
        let start = index;
        while index < chars.len()
            && !chars[index].is_whitespace()
            && !"{}[]:,".contains(chars[index])
        {
            if text[index..].starts_with("//") || text[index..].starts_with("/*") {
                break;
            }
            index += 1;
        }
        tokens.push(Token {
            kind: "literal".into(),
            value: chars[start..index].iter().collect(),
            start,
            end: index,
        });
    }
    Ok(tokens)
}

struct JsoncParser {
    tokens: Vec<Token>,
    pos: usize,
}

impl JsoncParser {
    fn new(text: &str) -> Result<JsoncParser> {
        let chars: Vec<char> = text.chars().collect();
        Ok(JsoncParser {
            tokens: lex_jsonc(&chars)?,
            pos: 0,
        })
    }

    fn current(&self) -> Result<&Token> {
        self.tokens
            .get(self.pos)
            .ok_or_else(|| setup_error("OpenCode 配置意外结束"))
    }

    fn take(&mut self, kind: &str) -> Result<Token> {
        let token = self.current()?.clone();
        if token.kind != kind {
            return Err(setup_error(format!(
                "OpenCode 配置结构无效：需要 {kind}，实际为 {}",
                token.kind
            )));
        }
        self.pos += 1;
        Ok(token)
    }

    fn value(&mut self) -> Result<Node> {
        let token = self.current()?.clone();
        match token.kind.as_str() {
            "{" => self.object(),
            "[" => {
                let start = self.take("[")?.start;
                if self.current()?.kind != "]" {
                    loop {
                        self.value()?;
                        if self.current()?.kind != "," {
                            break;
                        }
                        self.take(",")?;
                        if self.current()?.kind == "]" {
                            break;
                        }
                    }
                }
                let end = self.take("]")?.end;
                Ok(Node {
                    kind: "array".into(),
                    start,
                    end,
                    close_start: 0,
                    properties: Vec::new(),
                })
            }
            "string" => {
                self.pos += 1;
                Ok(Node {
                    kind: "scalar".into(),
                    start: token.start,
                    end: token.end,
                    close_start: 0,
                    properties: Vec::new(),
                })
            }
            "literal" => {
                if Json::parse(&token.value).is_err() {
                    return Err(setup_error(format!(
                        "OpenCode 配置包含无效字面量：{}",
                        token.value
                    )));
                }
                self.pos += 1;
                Ok(Node {
                    kind: "scalar".into(),
                    start: token.start,
                    end: token.end,
                    close_start: 0,
                    properties: Vec::new(),
                })
            }
            _ => Err(setup_error(format!(
                "OpenCode 配置包含无法识别的值：{}",
                token.value
            ))),
        }
    }

    fn object(&mut self) -> Result<Node> {
        let start = self.take("{")?.start;
        let mut properties: Vec<Property> = Vec::new();
        let mut previous_comma: Option<Token> = None;
        while self.current()?.kind != "}" {
            let key = self.take("string")?;
            if properties
                .iter()
                .any(|item: &Property| item.key == key.value)
            {
                return Err(setup_error(format!(
                    "OpenCode 配置包含重复键：{}",
                    key.value
                )));
            }
            self.take(":")?;
            let value = self.value()?;
            let mut prop = Property {
                key: key.value.clone(),
                start: key.start,
                end: value.end,
                value,
                comma_before: previous_comma.clone(),
                comma_after: None,
            };
            if self.current()?.kind == "," {
                let comma = self.take(",")?;
                prop.comma_after = Some(comma.clone());
                previous_comma = Some(comma);
                if self.current()?.kind == "}" {
                    properties.push(prop);
                    break;
                }
            } else {
                properties.push(prop);
                break;
            }
            properties.push(prop);
        }
        let close = self.take("}")?;
        Ok(Node {
            kind: "object".into(),
            start,
            end: close.end,
            close_start: close.start,
            properties,
        })
    }

    fn parse(&mut self) -> Result<Node> {
        let root = self.value()?;
        if self.pos != self.tokens.len() {
            return Err(setup_error("OpenCode 配置根对象之后还有无法识别的内容"));
        }
        if root.kind != "object" {
            return Err(setup_error("OpenCode 配置根节点必须是对象"));
        }
        Ok(root)
    }
}

fn find_property<'a>(node: &'a Node, name: &str) -> Option<&'a Property> {
    node.properties.iter().find(|item| item.key == name)
}

fn line_indent(chars: &[char], position: usize) -> String {
    let mut line_start = position;
    while line_start > 0 && chars[line_start - 1] != '\n' {
        line_start -= 1;
    }
    chars[line_start..position]
        .iter()
        .take_while(|ch| **ch == ' ' || **ch == '\t')
        .collect()
}

fn format_value(value: &Json, indent: &str) -> String {
    let rendered = value.dumps_indent2();
    rendered.replace('\n', &format!("\n{indent}"))
}

fn insert_property(chars: &[char], node: &Node, name: &str, value: &Json) -> Vec<char> {
    let close_indent = line_indent(chars, node.close_start);
    let child_indent = format!("{close_indent}  ");
    let rendered = format!(
        "{child_indent}{}: {}",
        Json::Str(name.to_string()).dumps(),
        format_value(value, &child_indent)
    );
    let prefix = match node.properties.last() {
        Some(last) if last.comma_after.is_none() => ",",
        _ => "",
    };
    let insertion = format!("{prefix}\n{rendered}\n{close_indent}");
    let mut out: Vec<char> = chars[..node.close_start].to_vec();
    out.extend(insertion.chars());
    out.extend_from_slice(&chars[node.close_start..]);
    out
}

fn remove_property(chars: &[char], prop: &Property) -> Vec<char> {
    if let Some(comma) = &prop.comma_after {
        return [&chars[..prop.start], &chars[comma.end..]].concat();
    }
    if let Some(comma) = &prop.comma_before {
        return [&chars[..comma.start], &chars[prop.end..]].concat();
    }
    [&chars[..prop.start], &chars[prop.end..]].concat()
}

fn is_owned_opencode_mcp(value_text: &str) -> bool {
    let normalized = value_text.to_lowercase().replace('\\', "/");
    normalized.contains("engramark")
        && (normalized.contains("mcp_server.py") || normalized.contains("bin/engramark"))
}

/// patch_opencode_config: add/replace/remove the mcp.engramark entry.
pub fn patch_opencode_config(text: &str, value: Option<&Json>) -> Result<String> {
    let mut text_owned = text.to_string();
    if text_owned.trim().is_empty() {
        text_owned = "{}\n".to_string();
    }
    let mut chars: Vec<char> = text_owned.chars().collect();
    let root = JsoncParser::new(&text_owned)?.parse()?;
    let mcp = find_property(&root, "mcp");
    if value.is_none() {
        let Some(mcp) = mcp else {
            return Ok(text_owned);
        };
        if mcp.value.kind != "object" {
            return Ok(text_owned);
        }
        let Some(target) = find_property(&mcp.value, "engramark") else {
            return Ok(text_owned);
        };
        let snippet: String = chars[target.value.start..target.value.end].iter().collect();
        if !is_owned_opencode_mcp(&snippet) {
            return Ok(text_owned);
        }
        let updated = remove_property(&chars, target);
        let result: String = updated.iter().collect();
        JsoncParser::new(&result)?.parse()?;
        return Ok(result);
    }
    let value = value.expect("checked");
    if mcp.is_none() {
        let updated = insert_property(
            &chars,
            &root,
            "mcp",
            &crate::jobject! {
                "engramark" => value.clone(),
            },
        );
        return Ok(updated.iter().collect());
    }
    let mcp = mcp.expect("checked");
    if mcp.value.kind != "object" {
        return Err(setup_error("OpenCode 配置的 mcp 不是对象，无法安全接入"));
    }
    let target = find_property(&mcp.value, "engramark");
    let Some(target) = target else {
        let updated = insert_property(&chars, &mcp.value, "engramark", value);
        return Ok(updated.iter().collect());
    };
    let snippet: String = chars[target.value.start..target.value.end].iter().collect();
    if !is_owned_opencode_mcp(&snippet) {
        return Err(setup_error(
            "OpenCode 已有同名 MCP，但不是 Engramark，已停止避免覆盖",
        ));
    }
    let indent = line_indent(&chars, target.start);
    let rendered = format_value(value, &indent);
    let mut out: Vec<char> = chars[..target.value.start].to_vec();
    out.extend(rendered.chars());
    out.extend_from_slice(&chars[target.value.end..]);
    chars = out;
    let result: String = chars.iter().collect();
    JsoncParser::new(&result)?.parse()?;
    Ok(result)
}

/// replace_block: idempotent managed-block replacement.
pub fn replace_block(text: &str, begin: &str, end: &str, body: Option<&str>) -> Result<String> {
    let begin_count = text.matches(begin).count();
    let end_count = text.matches(end).count();
    if begin_count != end_count || begin_count > 1 {
        return Err(setup_error(format!(
            "配置中的 Engramark 标记不完整或重复：{begin}"
        )));
    }
    let mut text = text.to_string();
    if begin_count == 1 {
        // \n?BEGIN.*?END\n? → "\n" (single, non-greedy, dotall)
        let begin_at = text.find(begin).expect("counted");
        let end_at = text[begin_at..]
            .find(end)
            .map(|at| begin_at + at + end.len())
            .expect("counted");
        let mut start = begin_at;
        if start > 0 && text.as_bytes()[start - 1] == b'\n' {
            start -= 1;
        }
        let mut finish = end_at;
        if finish < text.len() && text.as_bytes()[finish] == b'\n' {
            finish += 1;
        }
        text = format!("{}\n{}", &text[..start], &text[finish..]);
        text = text.trim_end().to_string();
    } else {
        text = text.trim_end().to_string();
    }
    if let Some(body) = body {
        let block = format!("{begin}\n{}\n{end}", body.trim_end());
        text = if text.is_empty() {
            block
        } else {
            format!("{text}\n\n{block}")
        };
    }
    Ok(format!("{}\n", text.trim_end()))
}

pub fn agent_block(data_home: &Path) -> String {
    AGENT_BLOCK_TEMPLATE.replace(
        "{DATA_HOME}",
        data_home
            .to_str()
            .expect("host-setup paths are prevalidated Unicode"),
    )
}

// --- Codex TOML surgery ---

fn find_section(lines: &[String], header: &str) -> Option<(usize, usize)> {
    let start = lines.iter().position(|line| line.trim() == header)?;
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim_start().starts_with('[') {
            end = index;
            break;
        }
    }
    Some((start, end))
}

fn memory_setting(text: &str) -> Option<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let (start, end) = find_section(&lines, "[features]")?;
    lines[start + 1..end]
        .iter()
        .find(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("memories") && trimmed[8..].trim_start().starts_with('=')
        })
        .map(|line| line.trim().to_string())
}

fn validate_toml(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    toml::from_str::<toml::Table>(text)
        .map(|_| ())
        .map_err(|err| setup_error(format!("Codex config.toml 不是有效 TOML：{err}")))
}

fn is_legacy_engramark_mcp(section: &str) -> bool {
    let normalized = section.to_lowercase().replace('\\', "/");
    normalized.contains("mcp_server.py")
        || normalized.contains("bin/engramark")
        || regex::Regex::new(r#"/engramark/(?:runtime/)?python(?:["']|$)"#)
            .map(|re| re.is_match(&normalized))
            .unwrap_or(false)
}

pub fn patch_codex_config(
    text: &str,
    app_root: &Path,
    data_home: &Path,
    install: bool,
    legacy_previous: Option<&str>,
) -> Result<String> {
    validate_toml(text)?;
    // Extract the previous memories setting recorded in a managed block.
    let mut previous_memory: Option<String> = None;
    if let Some(begin_at) = text.find(CODEX_MEMORY_BEGIN) {
        if let Some(end_at) = text[begin_at..]
            .find(CODEX_MEMORY_END)
            .map(|at| begin_at + at + CODEX_MEMORY_END.len())
        {
            let block = &text[begin_at..end_at];
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("# previous ") {
                    let rest = rest.trim();
                    if rest.starts_with("memories") && rest.contains('=') {
                        previous_memory = Some(rest.to_string());
                    }
                }
            }
        }
    }
    let text = replace_block(text, CODEX_MCP_BEGIN, CODEX_MCP_END, None)?;
    let text = replace_block(&text, CODEX_MEMORY_BEGIN, CODEX_MEMORY_END, None)?;
    // Remove legacy unmarked engramark blocks only when they set memories.
    let text = remove_legacy_marked_blocks(&text);
    let mut lines: Vec<String> = text.trim_end().lines().map(str::to_string).collect();
    if text.trim_end().is_empty() {
        lines = Vec::new();
    }
    if let Some((start, mut end)) = find_section(&lines, "[mcp_servers.engramark]") {
        while end < lines.len() {
            let stripped = lines[end].trim();
            if stripped.starts_with("[mcp_servers.engramark.") {
                if let Some((_, nested_end)) = find_section(&lines, stripped) {
                    end = nested_end;
                } else {
                    end += 1;
                }
            } else {
                break;
            }
        }
        let section = lines[start + 1..end].join("\n");
        if !is_legacy_engramark_mcp(&section) {
            return Err(setup_error(
                "Codex 已有同名 MCP，但不是 Engramark，已停止避免覆盖",
            ));
        }
        lines.drain(start..end);
    }
    if install {
        let features = find_section(&lines, "[features]");
        let (start, end) = match features {
            Some(bounds) => bounds,
            None => {
                if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push("[features]".to_string());
                (lines.len() - 1, lines.len())
            }
        };
        let mut previous: Option<String> =
            previous_memory.or_else(|| legacy_previous.map(str::to_string));
        let mut kept = vec![lines[start].clone()];
        for line in &lines[start + 1..end] {
            let trimmed = line.trim_start();
            if trimmed.starts_with("memories") && trimmed[8..].trim_start().starts_with('=') {
                previous = Some(line.clone());
            } else {
                kept.push(line.clone());
            }
        }
        while kept.len() > 1 && kept.last().is_some_and(|last| last.trim().is_empty()) {
            kept.pop();
        }
        let mut memory_lines = vec![CODEX_MEMORY_BEGIN.to_string()];
        if let Some(previous) = &previous {
            memory_lines.push(format!("# previous {}", previous.trim()));
        }
        memory_lines.push("memories = false".to_string());
        memory_lines.push(CODEX_MEMORY_END.to_string());
        kept.extend(memory_lines);
        lines.splice(start..end, kept);

        let binary = binary_path(app_root);
        let mcp_body = format!(
            "[mcp_servers.engramark]\ncommand = {}\nargs = [\"mcp\"]\n\n[mcp_servers.engramark.env]\nENGRAMARK_HOME = {}",
            Json::Str(binary.to_str().expect("host-setup paths are prevalidated Unicode").into()).dumps(),
            Json::Str(data_home.to_str().expect("host-setup paths are prevalidated Unicode").into()).dumps(),
        );
        let text = lines.join("\n").trim_end().to_string() + "\n";
        let result = replace_block(&text, CODEX_MCP_BEGIN, CODEX_MCP_END, Some(&mcp_body))?;
        validate_toml(&result)?;
        return Ok(result);
    }
    if let Some(previous) = previous_memory {
        let current = lines.join("\n");
        if memory_setting(&current).is_none() {
            match find_section(&lines, "[features]") {
                None => {
                    if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                        lines.push(String::new());
                    }
                    lines.push("[features]".to_string());
                    lines.push(previous);
                }
                Some((_, end)) => {
                    lines.insert(end, previous);
                }
            }
        }
    }
    let result = lines.join("\n").trim_end().to_string() + "\n";
    validate_toml(&result)?;
    Ok(result)
}

/// Legacy unmarked blocks: `# engramark-begin...\n(body)# engramark-end\n?` —
/// removed only when the body contains "memories".
fn remove_legacy_marked_blocks(text: &str) -> String {
    let mut text = text.to_string();
    let mut scan_from = 0usize;
    while let Some(begin_at) = text[scan_from..]
        .find("# engramark-begin")
        .map(|at| scan_from + at)
    {
        let line_end = text[begin_at..]
            .find('\n')
            .map(|at| begin_at + at + 1)
            .unwrap_or(text.len());
        let Some(end_at) = text[line_end..]
            .find("# engramark-end")
            .map(|at| line_end + at)
        else {
            break;
        };
        let body = &text[line_end..end_at];
        let mut finish = end_at + "# engramark-end".len();
        if finish < text.len() && text.as_bytes()[finish] == b'\n' {
            finish += 1;
        }
        if body.contains("memories") {
            let mut start = begin_at;
            if start > 0 && text.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            text = format!("{}\n{}", &text[..start], &text[finish..]);
            scan_from = start;
        } else {
            scan_from = finish;
        }
    }
    text
}

pub fn patch_codex_project_config(
    text: &str,
    project_root: &Path,
    install: bool,
) -> Result<String> {
    validate_toml(text)?;
    let had_managed = text.contains(CODEX_PROJECT_BEGIN) || text.contains(CODEX_PROJECT_END);
    let text = replace_block(text, CODEX_PROJECT_BEGIN, CODEX_PROJECT_END, None)?;
    if !install {
        validate_toml(&text)?;
        return Ok(text);
    }
    let lines: Vec<String> = text.trim_end().lines().map(str::to_string).collect();
    if find_section(&lines, "[mcp_servers.engramark]").is_some() && !had_managed {
        return Err(setup_error(
            "项目配置已经定义 mcp_servers.engramark；为避免覆盖，请先手工处理该段",
        ));
    }
    let body = format!(
        "[mcp_servers.engramark]\ncwd = {}",
        Json::Str(
            project_root
                .to_str()
                .expect("host-setup paths are prevalidated Unicode")
                .into()
        )
        .dumps()
    );
    let result = replace_block(&text, CODEX_PROJECT_BEGIN, CODEX_PROJECT_END, Some(&body))?;
    validate_toml(&result)?;
    Ok(result)
}

// --- hooks.json ---

pub fn render_hooks(app_root: &Path, data_home: &Path) -> Result<Json> {
    let mut payload = Json::parse(HOOKS_TEMPLATE)
        .map_err(|err| setup_error(format!("内置 hooks.json 模板非法：{err}")))?;
    let legacy = "$HOME/engramark";
    let program = app_root
        .to_str()
        .expect("host-setup paths are prevalidated Unicode");
    let Some(hooks) = payload.get("hooks").and_then(Json::as_object) else {
        return Ok(payload);
    };
    let mut new_hooks = Vec::new();
    for (event, groups) in hooks {
        let Some(groups) = groups.as_array() else {
            continue;
        };
        let mut new_groups = Vec::new();
        for group in groups {
            let Some(handlers) = group.get("hooks").and_then(Json::as_array) else {
                new_groups.push(group.clone());
                continue;
            };
            let mut new_handlers = Vec::new();
            for handler in handlers {
                let mut handler = handler.clone();
                if let Some(command) = handler.get("command").and_then(Json::as_str) {
                    let command = command.replace(legacy, program);
                    let command = if cfg!(windows) {
                        format!(
                            "\"{}\" {}",
                            binary_path(app_root)
                                .to_str()
                                .expect("host-setup paths are prevalidated Unicode"),
                            command_args_after_binary(&command)
                        )
                    } else {
                        format!(
                            "/usr/bin/env ENGRAMARK_HOME={} {}",
                            shell_quote(
                                data_home
                                    .to_str()
                                    .expect("host-setup paths are prevalidated Unicode")
                            ),
                            command
                        )
                    };
                    set_field(&mut handler, "command", Json::Str(command));
                    if cfg!(windows) {
                        let win_command = format!(
                            "set \"ENGRAMARK_HOME={}\" && \"{}\" {}",
                            data_home
                                .to_str()
                                .expect("host-setup paths are prevalidated Unicode"),
                            binary_path(app_root)
                                .to_str()
                                .expect("host-setup paths are prevalidated Unicode"),
                            command_args_after_binary(
                                handler.get("command").and_then(Json::as_str).unwrap_or("")
                            ),
                        );
                        set_field(&mut handler, "commandWindows", Json::Str(win_command));
                    }
                }
                new_handlers.push(handler);
            }
            let mut group = group.clone();
            set_field(&mut group, "hooks", Json::Array(new_handlers));
            new_groups.push(group);
        }
        new_hooks.push((event.clone(), Json::Array(new_groups)));
    }
    if let Json::Object(ref mut pairs) = payload {
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == "hooks") {
            slot.1 = Json::Object(new_hooks);
        }
    }
    Ok(payload)
}

fn command_args_after_binary(command: &str) -> String {
    // Template commands look like "$HOME/engramark/bin/engramark" hook <event>;
    // the render step keeps only the arguments after the binary token.
    match command.find("\" ") {
        Some(at) => command[at + 2..].to_string(),
        None => command.to_string(),
    }
}

fn set_field(target: &mut Json, key: &str, value: Json) {
    if let Json::Object(ref mut pairs) = target {
        if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            pairs.push((key.to_string(), value));
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn is_engramark_hook(item: &Json) -> Result<bool> {
    if !item.is_object() {
        return Err(setup_error("Codex hooks.json 的处理器必须是对象"));
    }
    let Some(command) = item.get("command").and_then(Json::as_str) else {
        if item.get("command").is_some() {
            return Err(setup_error("Codex hooks.json 的 command 必须是字符串"));
        }
        return Ok(false);
    };
    let normalized = command.to_lowercase().replace('\\', "/");
    Ok(normalized.contains("engramark")
        && (normalized.contains("/adapters/codex/hooks/") || normalized.contains(" hook codex-")))
}

pub fn patch_hooks(text: &str, app_root: &Path, data_home: &Path, install: bool) -> Result<String> {
    let payload = if text.trim().is_empty() {
        Json::Object(Vec::new())
    } else {
        Json::parse(text)
            .map_err(|err| setup_error(format!("Codex hooks.json 不是有效 JSON：{err}")))?
    };
    if !payload.is_object() {
        return Err(setup_error("Codex hooks.json 根节点必须是对象"));
    }
    let hooks_value = payload
        .get("hooks")
        .cloned()
        .unwrap_or(Json::Object(Vec::new()));
    if !hooks_value.is_object() {
        return Err(setup_error("Codex hooks.json 的 hooks 必须是对象"));
    }
    let ours = render_hooks(app_root, data_home)?;
    let mut new_hooks: Vec<(String, Json)> = Vec::new();
    if let Json::Object(events) = hooks_value.clone() {
        for (event, groups) in events {
            let Some(groups) = groups.as_array() else {
                return Err(setup_error(format!(
                    "Codex hooks.json 的 {event} 必须是数组"
                )));
            };
            let mut kept: Vec<Json> = Vec::new();
            for group in groups {
                if !group.is_object() {
                    return Err(setup_error(format!(
                        "Codex hooks.json 的 {event} 分组结构无效"
                    )));
                }
                let handlers = group
                    .get("hooks")
                    .cloned()
                    .unwrap_or(Json::Array(Vec::new()));
                let Some(handlers) = handlers.as_array() else {
                    return Err(setup_error(format!(
                        "Codex hooks.json 的 {event} 分组结构无效"
                    )));
                };
                let mut remaining = Vec::new();
                for handler in handlers {
                    if !is_engramark_hook(handler)? {
                        remaining.push(handler.clone());
                    }
                }
                if remaining.len() == handlers.len() {
                    kept.push(group.clone());
                } else if !remaining.is_empty() {
                    let mut group = group.clone();
                    set_field(&mut group, "hooks", Json::Array(remaining));
                    kept.push(group);
                }
            }
            if !kept.is_empty() {
                new_hooks.push((event.clone(), Json::Array(kept)));
            }
        }
    }
    let mut new_payload = payload.clone();
    if install {
        if let Json::Object(ours_events) = ours
            .get("hooks")
            .cloned()
            .unwrap_or(Json::Object(Vec::new()))
        {
            for (event, groups) in ours_events {
                if let Some(slot) = new_hooks.iter_mut().find(|(k, _)| *k == event) {
                    if let (Json::Array(existing), Json::Array(new)) = (&mut slot.1, &groups) {
                        existing.extend(new.iter().cloned());
                    }
                } else {
                    new_hooks.push((event, groups));
                }
            }
        }
        if new_hooks.is_empty() {
            remove_field(&mut new_payload, "hooks");
        } else {
            set_field(&mut new_payload, "hooks", Json::Object(new_hooks));
        }
        if new_payload.get("description").is_none() {
            if let Some(description) = ours.get("description") {
                set_field(&mut new_payload, "description", description.clone());
            }
        }
    } else {
        if new_hooks.is_empty() {
            remove_field(&mut new_payload, "hooks");
        } else {
            set_field(&mut new_payload, "hooks", Json::Object(new_hooks));
        }
        if new_payload.get("description") == ours.get("description").cloned().as_ref() {
            remove_field(&mut new_payload, "description");
        }
    }
    Ok(format!("{}\n", new_payload.dumps_indent2()))
}

fn remove_field(target: &mut Json, key: &str) {
    if let Json::Object(ref mut pairs) = target {
        pairs.retain(|(k, _)| k != key);
    }
}

// --- OpenCode plugin ---

fn is_owned_opencode_plugin(text: &str) -> bool {
    text.starts_with("// engramark-managed-opencode-plugin-v1")
        || text.starts_with("// engramark-managed-opencode-plugin-v2")
        || text.starts_with("// engramark-managed-opencode-plugin-v3")
        || text.starts_with("// engramark-managed-opencode-plugin-v4")
        || text.starts_with("// Engramark OpenCode 适配器（legacy 安全版）。")
}

fn render_opencode_plugin(app_root: &Path, data_home: &Path) -> Result<Vec<u8>> {
    let app_marker = "const MANAGED_APP_ROOT = null";
    let data_marker = "const MANAGED_DATA_HOME = null";
    if OPENCODE_PLUGIN.matches(app_marker).count() != 1
        || OPENCODE_PLUGIN.matches(data_marker).count() != 1
    {
        return Err(setup_error("OpenCode 插件缺少唯一的安装路径占位符"));
    }
    let rendered = OPENCODE_PLUGIN
        .replace(
            app_marker,
            &format!(
                "const MANAGED_APP_ROOT = {}",
                Json::Str(
                    app_root
                        .to_str()
                        .expect("host-setup paths are prevalidated Unicode")
                        .into()
                )
                .dumps()
            ),
        )
        .replace(
            data_marker,
            &format!(
                "const MANAGED_DATA_HOME = {}",
                Json::Str(
                    data_home
                        .to_str()
                        .expect("host-setup paths are prevalidated Unicode")
                        .into()
                )
                .dumps()
            ),
        );
    Ok(rendered.into_bytes())
}

// --- edits application with backup and rollback ---

fn edit_target(path: &Path) -> Result<PathBuf> {
    if !crate::paths::is_link_like(path) {
        if path.exists() && !path.is_file() {
            return Err(setup_error(format!(
                "宿主配置路径不是普通文件：{}",
                path.display()
            )));
        }
        return Ok(path.to_path_buf());
    }
    let target = std::fs::canonicalize(path)
        .map_err(|_| setup_error(format!("宿主配置是断开的符号链接：{}", path.display())))?;
    if !target.is_file() {
        return Err(setup_error(format!(
            "宿主配置链接目标不是普通文件：{}",
            path.display()
        )));
    }
    Ok(target)
}

fn text_edit(path: &Path, transform: impl Fn(&str) -> Result<String>) -> Result<Edit> {
    let path = edit_target(path)?;
    let before = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|_| setup_error(format!("无法按 UTF-8 读取宿主配置：{}", path.display())))?
    } else {
        String::new()
    };
    let after = transform(&before)?;
    Ok(Edit {
        path,
        content: Some(after.into_bytes()),
    })
}

fn sync_dir(path: &Path) {
    let _ = crate::durable_fs::fsync_dir(path);
}

fn apply_edits(edits: &[Edit], data_home: &Path) -> Result<()> {
    let mut paths: Vec<&PathBuf> = edits.iter().map(|edit| &edit.path).collect();
    paths.sort();
    paths.dedup();
    if paths.len() != edits.len() {
        return Err(setup_error(
            "多个宿主配置指向同一个文件，无法安全地分别修改",
        ));
    }
    let edits: Vec<Edit> = edits
        .iter()
        .filter(|edit| {
            let current = if edit.path.exists() {
                std::fs::read(&edit.path).ok()
            } else {
                None
            };
            current != edit.content
        })
        .cloned()
        .collect();
    if edits.is_empty() {
        return Ok(());
    }
    let originals: Vec<(PathBuf, Option<Vec<u8>>)> = edits
        .iter()
        .map(|edit| {
            (
                edit.path.clone(),
                if edit.path.exists() {
                    std::fs::read(&edit.path).ok()
                } else {
                    None
                },
            )
        })
        .collect();
    let stamp = format!(
        "{}-{}",
        crate::clock::clock().unix_seconds() as i64,
        &crate::clock::clock().uuid4().replace('-', "")[..8]
    );
    let backup = data_home.join("state").join("install-backups").join(&stamp);
    crate::durable_fs::create_dir_all_private(&backup)
        .map_err(|err| setup_error(err.to_string()))?;
    for path in [
        data_home.to_path_buf(),
        data_home.join("state"),
        data_home.join("state").join("install-backups"),
        backup.clone(),
    ] {
        crate::durable_fs::chmod_private(&path, true)
            .map_err(|err| setup_error(err.to_string()))?;
    }
    let mut index = Vec::new();
    for (number, (path, content)) in originals.iter().enumerate() {
        let mut item = crate::jobject! {
            "path" => path.to_str().expect("host-setup paths are prevalidated Unicode"),
            "existed" => content.is_some(),
        };
        if let Some(content) = content {
            let name = format!("{number:02}.bak");
            crate::durable_fs::atomic_write_bytes(&backup.join(&name), content)
                .map_err(|err| setup_error(err.to_string()))?;
            set_field(&mut item, "backup", Json::Str(name));
        }
        index.push(item);
    }
    crate::durable_fs::atomic_write_bytes(
        &backup.join("manifest.json"),
        format!("{}\n", Json::Array(index).dumps_indent2()).as_bytes(),
    )
    .map_err(|err| setup_error(err.to_string()))?;
    sync_dir(&backup);
    let result = (|| -> std::result::Result<(), String> {
        for edit in &edits {
            match &edit.content {
                None => {
                    let existed = edit.path.exists() || crate::paths::is_link_like(&edit.path);
                    let _ = std::fs::remove_file(&edit.path);
                    if existed {
                        if let Some(parent) = edit.path.parent() {
                            sync_dir(parent);
                        }
                    }
                }
                Some(content) => crate::durable_fs::atomic_write_bytes(&edit.path, content)
                    .map_err(|err| err.to_string())?,
            }
        }
        Ok(())
    })();
    if let Err(failure) = result {
        for (path, content) in &originals {
            match content {
                None => {
                    let _ = std::fs::remove_file(path);
                }
                Some(content) => {
                    let _ = crate::durable_fs::atomic_write_bytes(path, content);
                }
            }
        }
        return Err(setup_error(failure));
    }
    Ok(())
}

fn opencode_mcp_value(app_root: &Path, data_home: &Path) -> Json {
    crate::jobject! {
        "type" => "local",
        "command" => Json::Array(vec![
            Json::Str(binary_path(app_root).to_str().expect("host-setup paths are prevalidated Unicode").into()),
            Json::Str("mcp".into()),
        ]),
        "enabled" => true,
        "environment" => crate::jobject! {
            "ENGRAMARK_HOME" => data_home.to_str().expect("host-setup paths are prevalidated Unicode"),
        },
    }
}

pub fn build_edits(
    home: &Path,
    app_root: &Path,
    data_home: &Path,
    install: bool,
    codex: bool,
    opencode: bool,
) -> Result<Vec<Edit>> {
    let mut edits: Vec<Edit> = Vec::new();
    if codex {
        let codex_home = std::env::var("CODEX_HOME")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| crate::paths::expand_user(&value))
            .map(|path| crate::paths::resolve_lenient(&path))
            .unwrap_or_else(|| home.join(".codex"));
        let codex_config = codex_home.join("config.toml");
        let codex_agents = codex_home.join("AGENTS.md");
        let codex_hooks = codex_home.join("hooks.json");
        if install || codex_config.exists() {
            let legacy_backup = PathBuf::from(format!("{}.engramark-bak", codex_config.display()));
            let legacy_previous = if install && legacy_backup.exists() {
                std::fs::read_to_string(&legacy_backup)
                    .ok()
                    .and_then(|text| memory_setting(&text))
            } else {
                None
            };
            edits.push(text_edit(&codex_config, |value| {
                patch_codex_config(
                    value,
                    app_root,
                    data_home,
                    install,
                    legacy_previous.as_deref(),
                )
            })?);
        }
        if install || codex_agents.exists() {
            edits.push(text_edit(&codex_agents, |value| {
                let block;
                let body = if install {
                    block = agent_block(data_home);
                    Some(block.as_str())
                } else {
                    None
                };
                replace_block(value, AGENT_BEGIN, AGENT_END, body)
            })?);
        }
        if install || codex_hooks.exists() {
            edits.push(text_edit(&codex_hooks, |value| {
                patch_hooks(value, app_root, data_home, install)
            })?);
        }
    }
    if opencode {
        let config_home = home.join(".config").join("opencode");
        let candidates = [
            config_home.join("opencode.jsonc"),
            config_home.join("opencode.json"),
        ];
        let existing: Vec<&PathBuf> = candidates.iter().filter(|path| path.exists()).collect();
        if existing.len() > 1 {
            return Err(setup_error(
                "OpenCode 同时存在 opencode.jsonc 和 opencode.json，无法确定生效配置",
            ));
        }
        let config = existing
            .first()
            .map(|path| (*path).clone())
            .unwrap_or_else(|| candidates[0].clone());
        let value = opencode_mcp_value(app_root, data_home);
        if install || config.exists() {
            edits.push(text_edit(&config, |text| {
                patch_opencode_config(text, if install { Some(&value) } else { None })
            })?);
        }
        let open_agents = config_home.join("AGENTS.md");
        if install || open_agents.exists() {
            edits.push(text_edit(&open_agents, |text| {
                let block;
                let body = if install {
                    block = agent_block(data_home);
                    Some(block.as_str())
                } else {
                    None
                };
                replace_block(text, AGENT_BEGIN, AGENT_END, body)
            })?);
        }
        let plugin = config_home.join("plugins").join("engramark.js");
        if crate::paths::is_link_like(&plugin) {
            if install {
                return Err(setup_error("OpenCode 同名插件是符号链接，拒绝覆盖"));
            }
        } else if plugin.exists() && !plugin.is_file() {
            if install {
                return Err(setup_error("OpenCode 同名插件路径不是普通文件"));
            }
        } else {
            let plugin_exists = plugin.exists();
            let existing = if plugin_exists {
                String::from_utf8_lossy(&std::fs::read(&plugin).unwrap_or_default()).into_owned()
            } else {
                String::new()
            };
            let owned = !existing.is_empty() && is_owned_opencode_plugin(&existing);
            if install && plugin_exists && !owned {
                return Err(setup_error(
                    "OpenCode 已有同名插件但不属于 Engramark，拒绝覆盖",
                ));
            }
            if install {
                edits.push(Edit {
                    path: plugin,
                    content: Some(render_opencode_plugin(app_root, data_home)?),
                });
            } else if owned {
                edits.push(Edit {
                    path: plugin,
                    content: None,
                });
            }
        }
    }
    Ok(edits)
}

pub fn build_project_edit(project_root: &Path, install: bool) -> Result<Edit> {
    let config = project_root.join(".codex").join("config.toml");
    if !install && !config.exists() {
        return Ok(Edit {
            path: config,
            content: None,
        });
    }
    text_edit(&config, |value| {
        patch_codex_project_config(value, project_root, install)
    })
}

pub fn run_cli(command: &crate::cli::Command) -> Result<()> {
    let Some(args) = crate::cli::host_setup_args(command) else {
        return Ok(());
    };
    let layout = Layout::discover();
    let resolve_arg = |value: Option<&str>| {
        value
            .map(crate::paths::expand_user)
            .map(|path| crate::paths::resolve_lenient(&path))
    };
    let home = resolve_arg(args.home.as_deref())
        .unwrap_or_else(|| crate::paths::resolve_lenient(&crate::paths::home_dir()));
    let app_root = resolve_arg(args.app_root.as_deref())
        .unwrap_or_else(|| home.join(".local").join("share").join("engramark"));
    let data_home =
        resolve_arg(args.data_home.as_deref()).unwrap_or_else(|| home.join("engramark"));
    for path in [&home, &app_root, &data_home] {
        crate::paths::require_unicode(path).map_err(|err| setup_error(err.to_string()))?;
    }
    if !app_root.starts_with(&home) || !data_home.starts_with(&home) {
        return Err(setup_error("程序目录和记忆目录必须位于用户目录内"));
    }
    if app_root == data_home || app_root.starts_with(&data_home) || data_home.starts_with(&app_root)
    {
        return Err(setup_error("程序目录与记忆目录不能相同或互相嵌套"));
    }
    if args.action.starts_with("project-") {
        let Some(project) = args.project.as_deref() else {
            return Err(setup_error("项目操作必须提供 --project"));
        };
        let project_root = std::fs::canonicalize(crate::paths::expand_user(project))
            .map_err(|_| setup_error("--project 必须指向存在的具体项目目录"))?;
        crate::paths::require_unicode(&project_root).map_err(|err| setup_error(err.to_string()))?;
        let broad = [
            PathBuf::from("/"),
            home.clone(),
            home.join("Desktop"),
            home.join("Documents"),
            home.join("Downloads"),
            crate::paths::temp_dir(),
            app_root.clone(),
            data_home.clone(),
        ];
        if !project_root.is_dir()
            || broad.iter().any(|b| b == &project_root)
            || app_root.starts_with(&project_root)
            || project_root.starts_with(&app_root)
            || data_home.starts_with(&project_root)
            || project_root.starts_with(&data_home)
        {
            return Err(setup_error("--project 必须指向具体项目目录"));
        }
        let install_project = args.action != "project-disable";
        let edit = build_project_edit(&project_root, install_project)?;
        if args.action != "project-check" {
            apply_edits(std::slice::from_ref(&edit), &data_home)?;
        }
        println!(
            "{}",
            crate::jobject! {
                "action" => args.action.clone(),
                "project" => project_root.to_str().expect("host-setup paths are prevalidated Unicode"),
                "key" => "mcp_servers.engramark.cwd",
                "files" => Json::Array(vec![Json::Str(edit.path.to_str().expect("host-setup paths are prevalidated Unicode").into())]),
            }
            .dumps()
        );
        return Ok(());
    }
    let codex = args.codex == "yes" || (args.codex == "auto" && home.join(".codex").exists());
    let opencode = args.opencode == "yes"
        || (args.opencode == "auto" && home.join(".config").join("opencode").exists());
    let install = args.action != "uninstall";
    let edits = build_edits(&home, &app_root, &data_home, install, codex, opencode)?;
    if !edits.is_empty() && args.action != "check" {
        apply_edits(&edits, &data_home)?;
    }
    println!(
        "{}",
        crate::jobject! {
            "action" => args.action.clone(),
            "codex" => codex,
            "opencode" => opencode,
            "files" => Json::Array(edits.iter().map(|edit| Json::Str(edit.path.to_str().expect("host-setup paths are prevalidated Unicode").into())).collect()),
        }
        .dumps()
    );
    let _ = layout;
    let _ = VERSION;
    Ok(())
}
