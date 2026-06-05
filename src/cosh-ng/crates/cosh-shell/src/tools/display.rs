use serde_json::Value;

use super::classification::is_shell_tool_name;

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

    if is_shell_tool_name(name) {
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
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        format!("{}...", trimmed.chars().take(max).collect::<String>())
    }
}

fn compact_json(input: &str) -> String {
    match serde_json::from_str::<Value>(input) {
        Ok(v) => {
            let s = v.to_string();
            truncate(&s, 120)
        }
        Err(_) => truncate(input, 120),
    }
}

#[cfg(test)]
mod tests;
