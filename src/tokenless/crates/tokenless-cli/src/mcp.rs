//! Minimal MCP (Model Context Protocol) server over stdio.
//!
//! Exposes `tokenless_retrieve` so an MCP-connected agent can recover stash
//! payloads (written by `compress-response`) on demand — the MCP analogue of
//! the `tokenless retrieve` CLI. Hand-rolled JSON-RPC keeps the zero-runtime-
//! dependency core path; no MCP SDK is pulled in.
//!
//! The server stays alive across per-request errors: a tool failure is
//! returned as an MCP tool result (`isError: true`), not a server crash. Only
//! stdin/stdout I/O failure terminates the loop.

use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::sync::Arc;

use tokenless_ccr::{StashStore, extract_hash, is_valid_hash};

use crate::open_stash_store;

/// MCP protocol version implemented.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP stdio loop until EOF.
///
/// Returns `Err` only on stdin/stdout I/O failure; per-request tool errors
/// are surfaced as MCP tool results (`isError`) so the client keeps talking.
pub fn serve() -> Result<(), (String, i32)> {
    // Open the stash store once for the server's lifetime — SqliteStore wraps
    // a Connection in a Mutex for shared long-lived use, so re-opening per
    // request (the old behaviour) re-ran PRAGMAs + CREATE TABLE/INDEX on every
    // retrieve. Fail open at startup: if the db is unavailable the server
    // still serves tools/list + protocol handshake; retrieve returns a clear
    // "stash unavailable" tool error.
    let store = open_stash_store(None);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut line = String::new();
    let mut stdin_lock = stdin.lock();
    loop {
        line.clear();
        match stdin_lock.read_line(&mut line) {
            Ok(0) => return Ok(()), // EOF — client disconnected
            Ok(_) => {}
            Err(e) => {
                return Err((format!("mcp stdin read failed: {e}"), 1));
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                // JSON-RPC 2.0 §5 requires a Parse Error (-32700) for
                // syntactically broken JSON. Only lines that look like JSON
                // (start with `{`) get the error — other non-JSON lines (e.g.
                // LSP-style `Content-Length:` headers, if a client ever sends
                // them) are skipped silently to avoid spamming the client.
                if trimmed.starts_with('{') {
                    let _ = writeln!(out, "{}", err(Value::Null, -32700, "Parse error"));
                    let _ = out.flush();
                }
                continue;
            }
        };
        // Notifications (no `id`) expect no response.
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let response = match method {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {
                        "name": "tokenless",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ),
            "ping" => ok(id, json!({})), // MCP liveness check (utility method)
            "tools/list" => ok(id, json!({"tools": [retrieve_tool()]})),
            "tools/call" => {
                let params = req.get("params").cloned().unwrap_or(Value::Null);
                ok(id, handle_tool_call(params, &store))
            }
            other => err(id, -32601, &format!("method not found: {other}")),
        };
        if writeln!(out, "{response}").is_err() {
            return Err(("mcp stdout write failed".to_string(), 1));
        }
        if out.flush().is_err() {
            return Err(("mcp stdout flush failed".to_string(), 1));
        }
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

fn err(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn retrieve_tool() -> Value {
    json!({
        "name": "tokenless_retrieve",
        "description": "Retrieve a stashed payload by its 24-hex BLAKE3 key. Call this when a \
                        compressed tool response contained a `<<tokenless:KEY>>` marker and you \
                        need the original (truncated) content back. Accepts a bare hash or text \
                        containing a marker.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "hash": {
                    "type": "string",
                    "description": "24-hex stash key, or text containing a <<tokenless:KEY>> marker."
                }
            },
            "required": ["hash"]
        }
    })
}

/// Dispatch a `tools/call` to the named tool. Returns the MCP
/// `CallToolResult` object (content + isError). The stash store is opened
/// once at `serve()` startup and passed in — see the comment there.
fn handle_tool_call(params: Value, store: &Option<Arc<dyn StashStore>>) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    match name {
        "tokenless_retrieve" => retrieve(args, store),
        other => tool_error(&format!("unknown tool: {other}")),
    }
}

fn retrieve(args: Value, store: &Option<Arc<dyn StashStore>>) -> Value {
    let hash = args.get("hash").and_then(|h| h.as_str()).unwrap_or("");
    if hash.is_empty() {
        return tool_error("missing required argument: hash");
    }
    // Extract the stash key once: accepts a bare 24-hex hash or a line
    // containing a <<tokenless:KEY>> marker. Reject malformed input before
    // the DB round-trip so a non-hash argument gets a clear format error
    // instead of a misleading "no stashed payload".
    let key = match extract_hash(hash) {
        Some(k) => k,
        None if is_valid_hash(hash) => hash,
        None => {
            return tool_error(&format!(
                "invalid stash hash: {:?} (expected 24 hex chars or a <<tokenless:HASH>> marker)",
                hash
            ));
        }
    };
    // The store was opened once at startup (fail-open: the specific cause was
    // logged to stderr there). If it's unavailable, every retrieve reports so.
    let Some(store) = store.as_ref() else {
        return tool_error("stash unavailable: no trusted home directory or cannot open stash db");
    };
    retrieve_from_store(store, key)
}

/// Core retrieve against an explicit store, by the exact stash key. Split out
/// so the dispatch logic is unit-testable without touching the real stash db
/// path resolution. `key` must already be the final 24-hex stash key; marker
/// extraction happens in `retrieve` so the key is not re-scanned here.
fn retrieve_from_store(store: &Arc<dyn StashStore>, key: &str) -> Value {
    match store.retrieve(key) {
        Ok(Some(payload)) => json!({
            "content": [{"type":"text","text":payload}],
            "isError": false
        }),
        Ok(None) => tool_error(&format!("no stashed payload for hash: {key}")),
        Err(e) => tool_error(&format!("stash retrieve failed: {e}")),
    }
}

fn tool_error(message: &str) -> Value {
    json!({
        "content": [{"type":"text","text":message}],
        "isError": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("tests/mcp_tests.rs");
}
