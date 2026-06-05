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

#[test]
fn truncate_preserves_utf8_boundaries() {
    let preview = truncate("内存占用分析建议".repeat(20).as_str(), 120);
    assert!(preview.ends_with("..."));
    assert!(preview.is_char_boundary(preview.len()));
}

#[test]
fn compact_json_preserves_utf8_boundaries() {
    let json = serde_json::json!({
        "query": "内存占用分析建议".repeat(20)
    })
    .to_string();
    let preview = compact_json(&json);
    assert!(preview.ends_with("..."));
    assert!(preview.is_char_boundary(preview.len()));
}
