use tokenless_ccr::InMemoryStore;

#[test]
fn tool_list_exposes_retrieve() {
    let list = retrieve_tool();
    assert_eq!(list["name"], json!("tokenless_retrieve"));
    assert!(list["inputSchema"]["properties"]["hash"]["type"].is_string());
}

#[test]
fn dispatch_unknown_tool_is_error() {
    let r = handle_tool_call(json!({"name":"nope","arguments":{}}), &None);
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
}

#[test]
fn retrieve_missing_hash_is_error() {
    let r = retrieve(json!({}), &None);
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
}

#[test]
fn retrieve_invalid_hash_format_is_error() {
    // A non-hash argument (e.g. a file path) gets a format error before
    // any DB round-trip, not a misleading "no stashed payload".
    let r = retrieve(json!({"hash": "/some/path"}), &None);
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid stash hash")
    );
}

#[test]
fn retrieve_round_trips_via_store() {
    let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
    let key = store.stash("payload-body").unwrap();
    let r = retrieve_from_store(&store, &key);
    assert_eq!(r["isError"], json!(false));
    assert_eq!(r["content"][0]["text"], json!("payload-body"));
}

#[test]
fn retrieve_accepts_marker_line() {
    let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
    let key = store.stash("dropped items").unwrap();
    let marker_line = format!("<... 5 items truncated, retrieve with <<tokenless:{key}>>");
    // Exercise the public entry point so marker extraction in `retrieve`
    // is covered, not just the bare-key path of `retrieve_from_store`.
    let r = retrieve(json!({"hash": marker_line}), &Some(store));
    assert_eq!(r["content"][0]["text"], json!("dropped items"));
}

#[test]
fn retrieve_missing_payload_is_error() {
    let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
    let r = retrieve_from_store(&store, "000000000000000000000000");
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no stashed payload")
    );
}

#[test]
fn ok_envelope_has_jsonrpc_and_id() {
    let result = ok(json!(42), json!({"status": "ok"}));
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 42);
    assert_eq!(result["result"]["status"], "ok");
    assert!(result.get("error").is_none());
}

#[test]
fn err_envelope_has_jsonrpc_and_error() {
    let result = err(json!(7), -32601, "method not found");
    assert_eq!(result["jsonrpc"], "2.0");
    assert_eq!(result["id"], 7);
    assert_eq!(result["error"]["code"], -32601);
    assert_eq!(result["error"]["message"], "method not found");
    assert!(result.get("result").is_none());
}

#[test]
fn retrieve_stash_unavailable_when_store_is_none() {
    let valid_hash = "abcdef0123456789abcdef01";
    let r = retrieve(json!({"hash": valid_hash}), &None);
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("stash unavailable")
    );
}

#[test]
fn handle_tool_call_dispatches_retrieve_with_store() {
    let store: Arc<dyn StashStore> = Arc::new(InMemoryStore::new());
    let key = store.stash("mcp-payload").unwrap();
    let params = json!({
        "name": "tokenless_retrieve",
        "arguments": {"hash": key}
    });
    let r = handle_tool_call(params, &Some(store));
    assert_eq!(r["isError"], json!(false));
    assert_eq!(r["content"][0]["text"], "mcp-payload");
}

#[test]
fn handle_tool_call_missing_name_is_error() {
    let r = handle_tool_call(json!({}), &None);
    assert_eq!(r["isError"], json!(true));
    assert!(
        r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
}

#[test]
fn tool_error_has_is_error_true() {
    let r = tool_error("something broke");
    assert_eq!(r["isError"], json!(true));
    assert_eq!(r["content"][0]["text"], "something broke");
    assert_eq!(r["content"][0]["type"], "text");
}
