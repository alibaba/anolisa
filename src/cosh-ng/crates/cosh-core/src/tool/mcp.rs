//! Minimal stdio MCP client support for dynamically discovered agent tools.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

use crate::config::McpServerConfig;

use super::{Tool, ToolContext, ToolKind, ToolRegistry, ToolResult};

const CLIENT_NAME: &str = "cosh-ng";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const MAX_TOOL_NAME_LEN: usize = 64;
const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_MCP_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TOOL_LIST_PAGES: usize = 100;

/// Connects to every configured trusted server and registers its discovered tools.
///
/// A failed server is logged and skipped so one optional integration does not stop
/// the agent from starting.
pub async fn register_configured_tools(
    registry: &mut ToolRegistry,
    servers: &HashMap<String, McpServerConfig>,
) {
    let mut names: Vec<_> = servers.keys().collect();
    names.sort();

    for server_name in names {
        let Some(config) = servers.get(server_name) else {
            continue;
        };
        match McpClient::connect(server_name, config).await {
            Ok((client, tools)) => {
                let client = Arc::new(client);
                for tool in tools {
                    if !tool_is_allowed(&tool.name, config.allowed_tools.as_deref()) {
                        tracing::info!(
                            server = server_name,
                            tool = tool.name,
                            "MCP tool excluded by allowlist"
                        );
                        continue;
                    }

                    let exposed_name = exposed_tool_name(server_name, &tool.name);
                    if registry.contains(&exposed_name) {
                        tracing::warn!(
                            server = server_name,
                            tool = tool.name,
                            exposed_name,
                            "skipping MCP tool because its exposed name collides"
                        );
                        continue;
                    }
                    registry.register(Box::new(McpTool {
                        exposed_name,
                        remote_name: tool.name,
                        description: tool.description,
                        input_schema: tool.input_schema,
                        client: Arc::clone(&client),
                    }));
                }
            }
            Err(error) => {
                tracing::warn!(server = server_name, %error, "failed to start MCP server");
            }
        }
    }
}

fn tool_is_allowed(tool_name: &str, allowed_tools: Option<&[String]>) -> bool {
    allowed_tools
        .map(|tools| tools.iter().any(|allowed| allowed == tool_name))
        .unwrap_or(true)
}

fn exposed_tool_name(server_name: &str, tool_name: &str) -> String {
    let mut name = format!(
        "mcp__{}__{}",
        sanitize_identifier(server_name),
        sanitize_identifier(tool_name)
    );
    if name.len() > MAX_TOOL_NAME_LEN {
        name.truncate(MAX_TOOL_NAME_LEN);
    }
    name
}

fn sanitize_identifier(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .collect()
}

#[derive(Debug)]
struct DiscoveredTool {
    name: String,
    description: String,
    input_schema: Value,
}

struct McpTool {
    exposed_name: String,
    remote_name: String,
    description: String,
    input_schema: Value,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Mcp
    }

    async fn invoke(&self, params: Value, _ctx: &ToolContext) -> Result<ToolResult, String> {
        self.client.call_tool(&self.remote_name, params).await
    }
}

struct McpClient {
    server_name: String,
    timeout: Duration,
    startup_timeout: Duration,
    connection: Mutex<StdioConnection>,
}

impl McpClient {
    async fn connect(
        server_name: &str,
        config: &McpServerConfig,
    ) -> Result<(Self, Vec<DiscoveredTool>), String> {
        if config.command.trim().is_empty() {
            return Err("MCP command must not be empty".to_string());
        }
        if config.timeout_ms == 0 {
            return Err("MCP timeout_ms must be greater than zero".to_string());
        }
        if config.startup_timeout_ms == 0 {
            return Err("MCP startup_timeout_ms must be greater than zero".to_string());
        }

        let connection = StdioConnection::spawn(config).await?;
        let client = Self {
            server_name: server_name.to_string(),
            timeout: Duration::from_millis(config.timeout_ms),
            startup_timeout: Duration::from_millis(config.startup_timeout_ms),
            connection: Mutex::new(connection),
        };

        client.initialize().await?;
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    async fn initialize(&self) -> Result<(), String> {
        self.request_with_timeout(
            self.startup_timeout,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION }
            }),
        )
        .await?;
        self.notify("notifications/initialized", json!({})).await
    }

    async fn list_tools(&self) -> Result<Vec<DiscoveredTool>, String> {
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        let mut tools = Vec::new();

        for _ in 0..MAX_TOOL_LIST_PAGES {
            let mut params = serde_json::Map::new();
            if let Some(ref cursor) = cursor {
                params.insert("cursor".to_string(), Value::String(cursor.clone()));
            }
            let result = self
                .request_with_timeout(self.startup_timeout, "tools/list", Value::Object(params))
                .await?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| "MCP tools/list response has no tools array".to_string())?;

            for raw_tool in page {
                let name = raw_tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "MCP tools/list item has no name".to_string())?;
                let description = raw_tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("No description provided by MCP server.");
                let input_schema = raw_tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
                tools.push(DiscoveredTool {
                    name: name.to_string(),
                    description: format!("{description} (from {} MCP server)", self.server_name),
                    input_schema,
                });
            }

            let next_cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(ToString::to_string);
            match next_cursor {
                Some(next_cursor) if seen_cursors.insert(next_cursor.clone()) => {
                    cursor = Some(next_cursor);
                }
                Some(next_cursor) => {
                    return Err(format!(
                        "MCP tools/list returned repeated cursor: {next_cursor}"
                    ));
                }
                None => return Ok(tools),
            }
        }

        Err(format!(
            "MCP tools/list exceeded maximum page count of {MAX_TOOL_LIST_PAGES}"
        ))
    }

    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult, String> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool_name, "arguments": arguments }),
            )
            .await?;
        Ok(format_tool_result(&result))
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with_timeout(self.timeout, method, params)
            .await
    }

    async fn request_with_timeout(
        &self,
        request_timeout: Duration,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let mut connection = self.connection.lock().await;
        timeout(request_timeout, connection.request(method, params))
            .await
            .map_err(|_| {
                format!(
                    "MCP request '{method}' timed out for server {}",
                    self.server_name
                )
            })?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let mut connection = self.connection.lock().await;
        timeout(self.timeout, connection.notify(method, params))
            .await
            .map_err(|_| {
                format!(
                    "MCP notification '{method}' timed out for server {}",
                    self.server_name
                )
            })?
    }
}

struct StdioConnection {
    // Keep the process handle alive for the lifetime of its stdio connection.
    #[allow(dead_code)]
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
}

impl StdioConnection {
    async fn spawn(config: &McpServerConfig) -> Result<Self, String> {
        let executable = expand_env_vars(&config.command);
        let args: Vec<_> = config.args.iter().map(|arg| expand_env_vars(arg)).collect();
        let mut command = Command::new(&executable);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        configure_child_environment(&mut command, &config.env)?;

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start MCP command '{executable}': {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "failed to capture MCP process stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to capture MCP process stdout".to_string())?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_request_id: 1,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.write_message(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        loop {
            let line = self
                .read_message()
                .await?
                .ok_or_else(|| "MCP server closed stdout before responding".to_string())?;
            let message: Value = serde_json::from_slice(&line)
                .map_err(|error| format!("MCP server emitted invalid JSON-RPC: {error}"))?;

            let messages = match message {
                Value::Array(messages) => messages,
                message => vec![message],
            };
            for message in messages {
                if message.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    let description = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown MCP server error");
                    return Err(format!("MCP request '{method}' failed: {description}"));
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| format!("MCP response to '{method}' has no result"));
            }
        }
    }

    /// Reads one newline-delimited MCP message without allowing a server to grow memory without
    /// bound before JSON parsing begins.
    async fn read_message(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut line = Vec::new();

        loop {
            let (consumed, has_newline) = {
                let buffer = self
                    .stdout
                    .fill_buf()
                    .await
                    .map_err(|error| format!("failed to read MCP response: {error}"))?;
                if buffer.is_empty() {
                    return if line.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(line))
                    };
                }

                let take = buffer
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(buffer.len(), |position| position + 1);
                if line.len() + take > MAX_MCP_MESSAGE_BYTES {
                    return Err(format!(
                        "MCP message exceeds maximum size of {MAX_MCP_MESSAGE_BYTES} bytes"
                    ));
                }
                line.extend_from_slice(&buffer[..take]);
                (take, line.last() == Some(&b'\n'))
            };
            self.stdout.consume(consumed);
            if has_newline {
                line.pop();
                return Ok(Some(line));
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn write_message(&mut self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message)
            .map_err(|error| format!("failed to encode MCP request: {error}"))?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .map_err(|error| format!("failed to write MCP request: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("failed to flush MCP request: {error}"))
    }
}

fn configure_child_environment(
    command: &mut Command,
    configured_env: &HashMap<String, String>,
) -> Result<(), String> {
    command.env_clear();
    for name in ["HOME", "PATH", "TMPDIR", "LANG"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for (name, value) in configured_env {
        if !is_valid_env_name(name) {
            return Err(format!("invalid MCP environment variable name: {name}"));
        }
        command.env(name, expand_env_vars(value));
    }
    Ok(())
}

fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('A'..='Z' | 'a'..='z' | '_'))
        && chars.all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_'))
}

fn expand_env_vars(value: &str) -> String {
    let mut expanded = value.to_string();
    while let Some(start) = expanded.find("${") {
        let Some(end_offset) = expanded[start..].find('}') else {
            break;
        };
        let end = start + end_offset;
        let variable = &expanded[start + 2..end];
        let replacement = std::env::var(variable).unwrap_or_default();
        expanded.replace_range(start..=end, &replacement);
    }
    expanded
}

fn format_tool_result(result: &Value) -> ToolResult {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts = Vec::new();

    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
                Some("image") => {
                    let mime_type = block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    parts.push(format!("[MCP image result: {mime_type}]"));
                }
                Some("audio") => {
                    let mime_type = block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    parts.push(format!("[MCP audio result: {mime_type}]"));
                }
                Some("resource_link") => {
                    let title = block
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("resource");
                    let uri = block
                        .get("uri")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown URI");
                    parts.push(format!("[MCP resource: {title} at {uri}]"));
                }
                Some("resource") => {
                    let resource = block.get("resource").unwrap_or(block);
                    if let Some(text) = resource.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    } else {
                        let mime_type = resource
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        parts.push(format!("[MCP embedded resource: {mime_type}]"));
                    }
                }
                _ => parts.push(block.to_string()),
            }
        }
    }
    if let Some(structured) = result.get("structuredContent") {
        parts.push(structured.to_string());
    }
    if parts.is_empty() {
        parts.push(result.to_string());
    }

    ToolResult {
        output: truncate_output(parts.join("\n")),
        is_error,
    }
}

fn truncate_output(output: String) -> String {
    if output.len() <= MAX_TOOL_OUTPUT_BYTES {
        return output;
    }

    let mut end = MAX_TOOL_OUTPUT_BYTES;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[MCP output truncated]", &output[..end])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn test_context() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("/tmp"),
            session_id: "test".to_string(),
            project_root: PathBuf::from("/tmp"),
        }
    }

    fn fake_server(script: &str) -> (tempfile::TempDir, McpServerConfig) {
        let dir = tempfile::TempDir::new().unwrap();
        let script_path = dir.path().join("fake-mcp.sh");
        std::fs::write(&script_path, script).unwrap();
        let config = McpServerConfig {
            command: "sh".to_string(),
            args: vec![script_path.to_string_lossy().to_string()],
            env: HashMap::new(),
            timeout_ms: 1_000,
            startup_timeout_ms: 1_000,
            allowed_tools: None,
        };
        (dir, config)
    }

    #[test]
    fn exposes_valid_provider_tool_name() {
        assert_eq!(
            exposed_tool_name("github tools", "read/issue"),
            "mcp__github_tools__read_issue"
        );
    }

    #[test]
    fn formats_error_result() {
        let result = format_tool_result(&json!({
            "isError": true,
            "content": [{"type": "text", "text": "not found"}]
        }));
        assert!(result.is_error);
        assert_eq!(result.output, "not found");
    }

    #[test]
    fn truncates_large_tool_output() {
        let result = format_tool_result(&json!({
            "content": [{"type": "text", "text": "x".repeat(MAX_TOOL_OUTPUT_BYTES + 1)}]
        }));
        assert!(result.output.ends_with("[MCP output truncated]"));
    }

    #[tokio::test]
    async fn discovers_and_invokes_stdio_tool() {
        let (_dir, config) = fake_server(
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '[{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fake","version":"1.0"}}}]'
      ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '[{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo input","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}}]}}]'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '[{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"called"}]}}]'
      ;;
  esac
done
"#,
        );
        let mut servers = HashMap::new();
        servers.insert("fake".to_string(), config);
        let mut registry = ToolRegistry::new();

        register_configured_tools(&mut registry, &servers).await;
        let tool = registry
            .get("mcp__fake__echo")
            .expect("discovered MCP tool");
        let result = tool
            .invoke(json!({"message": "hello"}), &test_context())
            .await
            .unwrap();
        assert_eq!(result.output, "called");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn rejects_repeated_tool_list_cursor() {
        let (_dir, config) = fake_server(
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[],"nextCursor":"again"}}\n' "$id"
      ;;
  esac
done
"#,
        );

        let error = match McpClient::connect("fake", &config).await {
            Ok(_) => panic!("repeated cursor should fail MCP tool discovery"),
            Err(error) => error,
        };
        assert!(error.contains("repeated cursor: again"));
    }

    #[tokio::test]
    async fn empty_allowlist_registers_no_tools() {
        let (_dir, mut config) = fake_server(
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}' ;;
    *'"method":"tools/list"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}' ;;
  esac
done
"#,
        );
        config.allowed_tools = Some(vec![]);
        let mut registry = ToolRegistry::new();
        let mut servers = HashMap::new();
        servers.insert("fake".to_string(), config);

        register_configured_tools(&mut registry, &servers).await;
        assert!(registry.get("mcp__fake__echo").is_none());
    }

    #[tokio::test]
    async fn rejects_oversized_mcp_message() {
        let (_dir, config) = fake_server(
            r#"#!/bin/sh
while IFS= read -r line; do
  head -c 1048577 /dev/zero | tr '\0' x
  printf '\n'
done
"#,
        );

        let error = match McpClient::connect("fake", &config).await {
            Ok(_) => panic!("oversized MCP message should fail"),
            Err(error) => error,
        };
        assert!(error.contains("MCP message exceeds maximum size"));
    }
}
