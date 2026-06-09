use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolColor {
    ReadOnly,
    Execute,
    Write,
    Dangerous,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ToolDisplayInfo {
    pub label: String,
    pub color: ToolColor,
    pub preview: String,
}

pub fn display_for_tool(name: &str, input_json: &str) -> ToolDisplayInfo {
    let parsed = serde_json::from_str::<Value>(input_json).ok();

    if is_bash_tool_name(name) {
        return display_bash(&parsed, input_json);
    }

    match name {
        "Read" | "read_file" => display_read(&parsed, input_json),
        "Write" | "write_file" => display_write(&parsed, input_json),
        "Edit" => display_edit(&parsed, input_json),
        "Grep" | "grep_search" => display_grep(&parsed, input_json),
        "Glob" => display_glob(&parsed, input_json),
        "LS" | "list_directory" => display_ls(&parsed, input_json),
        "read_many_files" => display_many_files(&parsed, input_json),
        "LSP" => display_lsp(&parsed, input_json),
        "WebFetch" => display_web_fetch(&parsed, input_json),
        "WebSearch" => display_web_search(&parsed, input_json),
        _ => ToolDisplayInfo {
            label: name.to_string(),
            color: ToolColor::Unknown,
            preview: compact_json(input_json),
        },
    }
}

fn is_bash_tool_name(name: &str) -> bool {
    matches!(
        name,
        "Bash"
            | "shell"
            | "run_shell_command"
            | "tool Bash"
            | "tool shell"
            | "tool run_shell_command"
    )
}

fn display_bash(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let command = parsed
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or(input_json);

    let color = if is_dangerous_command(command) {
        ToolColor::Dangerous
    } else {
        ToolColor::Execute
    };

    ToolDisplayInfo {
        label: "Bash".to_string(),
        color,
        preview: format!("$ {command}"),
    }
}

fn is_dangerous_command(command: &str) -> bool {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    tokens
        .iter()
        .any(|t| *t == "sudo" || *t == "rm" || *t == "kill")
}

fn display_read(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let file_path = str_field(parsed, "file_path").unwrap_or(input_json);

    let mut preview = file_path.to_string();
    if let Some(offset) = parsed
        .as_ref()
        .and_then(|v| v.get("offset"))
        .and_then(|v| v.as_u64())
    {
        let limit = parsed
            .as_ref()
            .and_then(|v| v.get("limit"))
            .and_then(|v| v.as_u64());
        match limit {
            Some(l) => preview = format!("{file_path} (lines {offset}..+{l})"),
            None => preview = format!("{file_path} (from line {offset})"),
        }
    }

    ToolDisplayInfo {
        label: "Read".to_string(),
        color: ToolColor::ReadOnly,
        preview,
    }
}

fn display_write(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let file_path = str_field(parsed, "file_path").unwrap_or(input_json);

    ToolDisplayInfo {
        label: "Write".to_string(),
        color: ToolColor::Write,
        preview: format!("{file_path} (new file)"),
    }
}

fn display_edit(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let file_path = str_field(parsed, "file_path").unwrap_or(input_json);
    let old = str_field(parsed, "old_string").unwrap_or("");
    let new = str_field(parsed, "new_string").unwrap_or("");

    let old_short = truncate(old, 30);
    let new_short = truncate(new, 30);
    let diff = if !old_short.is_empty() || !new_short.is_empty() {
        format!(" ({old_short} -> {new_short})")
    } else {
        String::new()
    };

    ToolDisplayInfo {
        label: "Edit".to_string(),
        color: ToolColor::Write,
        preview: format!("{file_path}{diff}"),
    }
}

fn display_grep(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let pattern = str_field(parsed, "pattern").unwrap_or("?");
    let path = str_field(parsed, "path").unwrap_or(input_json);

    ToolDisplayInfo {
        label: "Grep".to_string(),
        color: ToolColor::ReadOnly,
        preview: format!("/{pattern}/ in {path}"),
    }
}

fn display_glob(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let pattern = str_field(parsed, "pattern").unwrap_or(input_json);

    ToolDisplayInfo {
        label: "Glob".to_string(),
        color: ToolColor::ReadOnly,
        preview: pattern.to_string(),
    }
}

fn display_ls(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let path = str_field(parsed, "path")
        .or_else(|| str_field(parsed, "dir_path"))
        .unwrap_or(input_json);

    ToolDisplayInfo {
        label: "LS".to_string(),
        color: ToolColor::ReadOnly,
        preview: path.to_string(),
    }
}

fn display_many_files(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let preview = parsed
        .as_ref()
        .and_then(|v| v.get("paths"))
        .map(Value::to_string)
        .unwrap_or_else(|| compact_json(input_json));

    ToolDisplayInfo {
        label: "Read".to_string(),
        color: ToolColor::ReadOnly,
        preview,
    }
}

fn display_lsp(parsed: &Option<Value>, _input_json: &str) -> ToolDisplayInfo {
    let operation = str_field(parsed, "operation").unwrap_or("unknown");
    let file_path = str_field(parsed, "filePath").unwrap_or("?");
    let line = parsed
        .as_ref()
        .and_then(|v| v.get("line"))
        .and_then(|v| v.as_u64())
        .map(|l| l.to_string())
        .unwrap_or_else(|| "?".to_string());

    ToolDisplayInfo {
        label: format!("LSP {operation}"),
        color: ToolColor::ReadOnly,
        preview: format!("{file_path}:{line}"),
    }
}

fn display_web_fetch(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let url = str_field(parsed, "url").unwrap_or(input_json);

    ToolDisplayInfo {
        label: "WebFetch".to_string(),
        color: ToolColor::ReadOnly,
        preview: url.to_string(),
    }
}

fn display_web_search(parsed: &Option<Value>, input_json: &str) -> ToolDisplayInfo {
    let query = str_field(parsed, "query").unwrap_or(input_json);

    ToolDisplayInfo {
        label: "WebSearch".to_string(),
        color: ToolColor::ReadOnly,
        preview: format!("\"{query}\""),
    }
}

fn str_field<'a>(parsed: &'a Option<Value>, key: &str) -> Option<&'a str> {
    parsed.as_ref()?.get(key)?.as_str()
}

fn truncate(s: &str, max: usize) -> String {
    let trimmed = s.lines().next().unwrap_or(s);
    if trimmed.len() <= max {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..max])
    }
}

fn compact_json(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(v) => {
            let s = v.to_string();
            if s.len() > 120 {
                format!("{}...", &s[..120])
            } else {
                s
            }
        }
        Err(_) => {
            if input.len() > 120 {
                format!("{}...", &input[..120])
            } else {
                input.to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_simple_command() {
        let info = display_for_tool("Bash", r#"{"command":"ls -la"}"#);
        assert_eq!(info.label, "Bash");
        assert_eq!(info.preview, "$ ls -la");
        assert_eq!(info.color, ToolColor::Execute);
    }

    #[test]
    fn bash_aliases_use_canonical_display() {
        for alias in [
            "shell",
            "run_shell_command",
            "tool shell",
            "tool run_shell_command",
        ] {
            let info = display_for_tool(alias, r#"{"command":"pwd"}"#);
            assert_eq!(info.label, "Bash", "{alias}");
            assert_eq!(info.preview, "$ pwd", "{alias}");
            assert_eq!(info.color, ToolColor::Execute, "{alias}");
        }
    }

    #[test]
    fn bash_dangerous_sudo() {
        let info = display_for_tool("Bash", r#"{"command":"sudo apt install foo"}"#);
        assert_eq!(info.color, ToolColor::Dangerous);
        assert_eq!(info.preview, "$ sudo apt install foo");
    }

    #[test]
    fn bash_dangerous_rm() {
        let info = display_for_tool("Bash", r#"{"command":"rm -rf /tmp/x"}"#);
        assert_eq!(info.color, ToolColor::Dangerous);
    }

    #[test]
    fn bash_dangerous_kill() {
        let info = display_for_tool("Bash", r#"{"command":"kill -9 1234"}"#);
        assert_eq!(info.color, ToolColor::Dangerous);
    }

    #[test]
    fn read_simple() {
        let info = display_for_tool("Read", r#"{"file_path":"/tmp/foo.rs"}"#);
        assert_eq!(info.label, "Read");
        assert_eq!(info.preview, "/tmp/foo.rs");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn qwen_read_alias_uses_canonical_display() {
        let info = display_for_tool("read_file", r#"{"file_path":"/tmp/foo.rs"}"#);
        assert_eq!(info.label, "Read");
        assert_eq!(info.preview, "/tmp/foo.rs");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn read_with_range() {
        let info = display_for_tool(
            "Read",
            r#"{"file_path":"/tmp/foo.rs","offset":10,"limit":20}"#,
        );
        assert_eq!(info.preview, "/tmp/foo.rs (lines 10..+20)");
    }

    #[test]
    fn read_with_offset_only() {
        let info = display_for_tool("Read", r#"{"file_path":"/tmp/foo.rs","offset":5}"#);
        assert_eq!(info.preview, "/tmp/foo.rs (from line 5)");
    }

    #[test]
    fn write_tool() {
        let info = display_for_tool("Write", r#"{"file_path":"/tmp/new.rs","content":"hello"}"#);
        assert_eq!(info.label, "Write");
        assert_eq!(info.preview, "/tmp/new.rs (new file)");
        assert_eq!(info.color, ToolColor::Write);
    }

    #[test]
    fn qwen_write_file_alias_uses_canonical_display() {
        let info = display_for_tool(
            "write_file",
            r#"{"file_path":"/tmp/new.html","content":"<html></html>"}"#,
        );
        assert_eq!(info.label, "Write");
        assert_eq!(info.preview, "/tmp/new.html (new file)");
        assert_eq!(info.color, ToolColor::Write);
    }

    #[test]
    fn edit_tool() {
        let info = display_for_tool(
            "Edit",
            r#"{"file_path":"/tmp/x.rs","old_string":"foo","new_string":"bar"}"#,
        );
        assert_eq!(info.label, "Edit");
        assert_eq!(info.preview, "/tmp/x.rs (foo -> bar)");
        assert_eq!(info.color, ToolColor::Write);
    }

    #[test]
    fn grep_tool() {
        let info = display_for_tool("Grep", r#"{"pattern":"TODO","path":"src/"}"#);
        assert_eq!(info.label, "Grep");
        assert_eq!(info.preview, "/TODO/ in src/");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn qwen_grep_alias_uses_canonical_display() {
        let info = display_for_tool("grep_search", r#"{"pattern":"TODO","path":"src/"}"#);
        assert_eq!(info.label, "Grep");
        assert_eq!(info.preview, "/TODO/ in src/");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn glob_tool() {
        let info = display_for_tool("Glob", r#"{"pattern":"**/*.rs"}"#);
        assert_eq!(info.label, "Glob");
        assert_eq!(info.preview, "**/*.rs");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn qwen_ls_alias_uses_canonical_display() {
        let info = display_for_tool("list_directory", r#"{"path":"src"}"#);
        assert_eq!(info.label, "LS");
        assert_eq!(info.preview, "src");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn lsp_tool() {
        let info = display_for_tool(
            "LSP",
            r#"{"operation":"goToDefinition","filePath":"src/main.rs","line":42}"#,
        );
        assert_eq!(info.label, "LSP goToDefinition");
        assert_eq!(info.preview, "src/main.rs:42");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn web_fetch_tool() {
        let info = display_for_tool("WebFetch", r#"{"url":"https://example.com"}"#);
        assert_eq!(info.label, "WebFetch");
        assert_eq!(info.preview, "https://example.com");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn web_search_tool() {
        let info = display_for_tool("WebSearch", r#"{"query":"rust async"}"#);
        assert_eq!(info.label, "WebSearch");
        assert_eq!(info.preview, "\"rust async\"");
        assert_eq!(info.color, ToolColor::ReadOnly);
    }

    #[test]
    fn unknown_tool() {
        let info = display_for_tool("CustomTool", r#"{"x":1}"#);
        assert_eq!(info.label, "CustomTool");
        assert_eq!(info.color, ToolColor::Unknown);
        assert_eq!(info.preview, r#"{"x":1}"#);
    }

    #[test]
    fn malformed_json_fallback() {
        let info = display_for_tool("Bash", "not json at all");
        assert_eq!(info.label, "Bash");
        assert_eq!(info.preview, "$ not json at all");
        assert_eq!(info.color, ToolColor::Execute);
    }

    #[test]
    fn malformed_json_unknown_tool() {
        let info = display_for_tool("Foo", "broken{json");
        assert_eq!(info.label, "Foo");
        assert_eq!(info.color, ToolColor::Unknown);
        assert_eq!(info.preview, "broken{json");
    }
}
