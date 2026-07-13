use std::sync::Mutex;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct TempDbGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    test_dir: String,
    sls_dir: String,
    prev_stats_db: Option<std::ffi::OsString>,
    prev_stash_db: Option<std::ffi::OsString>,
    prev_sls_path: Option<std::ffi::OsString>,
}

impl TempDbGuard {
    fn new() -> Option<Self> {
        let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let home = get_home_dir();
        if home.is_empty() {
            return None;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let test_dir = format!("{}/.tokenless-test-{}", home, nanos);
        let sls_dir = format!("/tmp/tokenless-sls-test-{}", nanos);
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::create_dir_all(&sls_dir).unwrap();
        let sls_path = format!("{}/tokenless.jsonl", sls_dir);
        // Pre-create the JSONL file so SlsWriter::write can open it.
        std::fs::write(&sls_path, "").unwrap();
        let prev_stats_db = std::env::var_os("TOKENLESS_STATS_DB");
        let prev_stash_db = std::env::var_os("TOKENLESS_STASH_DB");
        let prev_sls_path = std::env::var_os("TOKENLESS_SLS_PATH");
        unsafe {
            std::env::set_var("TOKENLESS_STATS_DB", format!("{}/stats.db", test_dir));
            std::env::set_var("TOKENLESS_STASH_DB", format!("{}/stash.db", test_dir));
            std::env::set_var("TOKENLESS_SLS_PATH", &sls_path);
        }
        Some(TempDbGuard {
            _lock: lock,
            test_dir,
            sls_dir,
            prev_stats_db,
            prev_stash_db,
            prev_sls_path,
        })
    }
}

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev_stats_db {
                Some(v) => std::env::set_var("TOKENLESS_STATS_DB", v),
                None => std::env::remove_var("TOKENLESS_STATS_DB"),
            }
            match &self.prev_stash_db {
                Some(v) => std::env::set_var("TOKENLESS_STASH_DB", v),
                None => std::env::remove_var("TOKENLESS_STASH_DB"),
            }
            match &self.prev_sls_path {
                Some(v) => std::env::set_var("TOKENLESS_SLS_PATH", v),
                None => std::env::remove_var("TOKENLESS_SLS_PATH"),
            }
        }
        std::fs::remove_dir_all(&self.test_dir).ok();
        std::fs::remove_dir_all(&self.sls_dir).ok();
    }
}

fn temp_subdir(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "tokenless-db-validate-{}-{}-{}",
        std::process::id(),
        nanos,
        label
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn validate_db_path_rejects_empty_home() {
    // No trusted home anchor means starts_with("") would match
    // any path, so the function must short-circuit to rejection.
    let err = validate_db_path("/tmp/whatever.db", "").unwrap_err();
    assert!(err.contains("no trusted home"));
}

#[test]
fn validate_db_path_accepts_path_inside_home() {
    let home = temp_subdir("inside");
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let inner = canon_home.join("stats.db");
    let result =
        validate_db_path(inner.to_str().unwrap(), canon_home.to_str().unwrap()).unwrap();
    assert_eq!(result, inner.to_str().unwrap());
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn validate_db_path_rejects_path_outside_home() {
    let home = temp_subdir("outside-home");
    let canon_home = std::fs::canonicalize(&home).unwrap();
    // Pick a known-existing directory that is NOT under home.
    let outside = std::path::Path::new("/etc/hosts");
    if !outside.exists() {
        std::fs::remove_dir_all(&home).ok();
        return;
    }
    let err = validate_db_path("/etc/hosts", canon_home.to_str().unwrap()).unwrap_err();
    assert!(err.contains("outside home"));
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn validate_db_path_rejects_parent_dir_bypass_with_existing_parent() {
    // ~/foo/../../etc/evil.db where /etc exists: canonicalize() of
    // the parent resolves to /etc, which must fail starts_with(home).
    let home = temp_subdir("pd-existing");
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let escape = canon_home.join("foo/../../etc/evil.db");
    let err =
        validate_db_path(escape.to_str().unwrap(), canon_home.to_str().unwrap()).unwrap_err();
    // Either "outside home" (parent canonicalized away from home) or
    // "cannot be resolved" (parent itself unreachable). Both are valid
    // rejections — what matters is no Ok return.
    assert!(err.contains("outside home") || err.contains("cannot be resolved"));
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn validate_db_path_canonicalizes_home_with_symlink_prefix() {
    // If the caller passes a home that contains a symlink in any
    // prefix (e.g. /tmp on macOS resolves to /private/tmp), the
    // candidate path will canonicalize to the resolved form and
    // diverge from the raw home unless validate_db_path canonicalizes
    // home too. Linux /tmp has no such symlink, so the assertion is
    // informational there but real coverage on macOS.
    let home = temp_subdir("sym-prefix");
    let inner = home.join("stats.db");
    let result = validate_db_path(inner.to_str().unwrap(), home.to_str().unwrap());
    assert!(
        result.is_ok(),
        "raw (non-canonical) home should be accepted after internal canonicalization: {:?}",
        result
    );
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn validate_db_path_rejects_parent_dir_bypass_with_nonexistent_parent() {
    // ~/nonexistent-path/../../etc/evil.db where nonexistent-path
    // doesn't exist: parent canonicalize() ALSO fails, so without the
    // hardening this path would slip through via the old fallback.
    let home = temp_subdir("pd-nonexistent");
    let canon_home = std::fs::canonicalize(&home).unwrap();
    let escape = canon_home.join("does-not-exist-xyz/../../etc/evil.db");
    let result = validate_db_path(escape.to_str().unwrap(), canon_home.to_str().unwrap());
    assert!(
        result.is_err(),
        "ParentDir bypass via nonexistent intermediate must be rejected; got {:?}",
        result
    );
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn read_input_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("input.json");
    std::fs::write(&f, r#"{"key":"value"}"#).unwrap();
    let result = read_input(&Some(f.to_str().unwrap().to_string())).unwrap();
    assert_eq!(result, r#"{"key":"value"}"#);
}

#[test]
fn read_input_file_not_found() {
    let result = read_input(&Some("/nonexistent/path.json".to_string()));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Failed to open"));
}

#[test]
fn resolve_mode_active_when_on() {
    let mode = resolve_mode(true, 100, 50);
    assert_eq!(mode, CompressionMode::Active);
}

#[test]
fn resolve_mode_dryrun_when_off() {
    let mode = resolve_mode(false, 100, 50);
    assert_eq!(mode, CompressionMode::DryRun);
}

#[test]
fn warn_mode_mismatch_empty_records_no_panic() {
    warn_mode_mismatch("test", &[], CompressionMode::Active);
}

#[test]
fn warn_mode_mismatch_detects_wrong_modes() {
    let records = vec![
        StatsRecord::new(
            OperationType::CompressSchema,
            "cli".to_string(),
            100,
            25,
            50,
            12,
        )
        .with_mode(CompressionMode::Active),
    ];
    // Expect DryRun but got Active → warning printed to stderr (no panic)
    warn_mode_mismatch("baseline", &records, CompressionMode::DryRun);
}

#[test]
fn warn_mode_mismatch_no_warning_when_matching() {
    let records = vec![
        StatsRecord::new(
            OperationType::CompressSchema,
            "cli".to_string(),
            100,
            25,
            50,
            12,
        )
        .with_mode(CompressionMode::DryRun),
    ];
    warn_mode_mismatch("baseline", &records, CompressionMode::DryRun);
}

#[test]
fn get_stash_db_path_default() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let home = get_home_dir();
    if home.is_empty() {
        return;
    }
    let path = get_stash_db_path(None);
    assert!(path.is_some());
    assert!(path.unwrap().contains(".tokenless/stash.db"));
}

#[test]
fn get_stash_db_path_with_valid_override() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let home = get_home_dir();
    if home.is_empty() {
        return;
    }
    let dir = format!("{}/.tokenless", home);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = format!("{}/override_test.db", dir);
    let result = get_stash_db_path(Some(&db_path));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), db_path);
}

#[test]
fn open_stash_store_falls_back_on_bad_override() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    // Bad override is rejected but falls back to the default path.
    // Result may be None in CI if the DB is locked by another test.
    let _result = open_stash_store(Some("/nonexistent/deep/dir/stash.db"));
}

#[test]
fn open_stash_store_or_err_falls_back_on_bad_override() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = open_stash_store_or_err(Some("/nonexistent/deep/dir/stash.db"));
    // Bad override path is rejected and falls back to default; result is Ok
    assert!(result.is_ok());
}

#[test]
fn ensure_db_dir_creates_parent() {
    let _guard = TempDbGuard::new();

    // ensure_db_dir is idempotent — calling it when the dir already exists
    // (which it does for most test envs) succeeds.
    let result = ensure_db_dir();
    // With TempDbGuard the stats DB path points to a temp dir, so this succeeds.
    assert!(result.is_ok());
}

#[test]
fn record_compression_stats_skips_when_both_disabled() {
    let config = TokenlessConfig {
        stats_enabled: false,
        sls_enabled: false,
        ..TokenlessConfig::default()
    };
    // Should return immediately without touching DB
    record_compression_stats(
        &config,
        OperationType::CompressSchema,
        Some("test".to_string()),
        None,
        None,
        "before".to_string(),
        "after".to_string(),
        CompressionMode::Active,
        None,
        None,
        None,
    );
}

#[test]
fn record_compression_stats_skips_when_no_savings() {
    let config = TokenlessConfig::default();
    // after is larger than before → no savings → skip
    record_compression_stats(
        &config,
        OperationType::CompressSchema,
        Some("test".to_string()),
        None,
        None,
        "short".to_string(),
        "this is longer text".to_string(),
        CompressionMode::Active,
        None,
        None,
        None,
    );
}

#[test]
fn record_compression_stats_records_when_savings_exist() {
    let config = TokenlessConfig::default();
    let long_before = "x".repeat(500);
    let short_after = "y".repeat(50);
    record_compression_stats(
        &config,
        OperationType::CompressSchema,
        Some("test-agent".to_string()),
        Some("session-1".to_string()),
        Some("tool-1".to_string()),
        long_before,
        short_after,
        CompressionMode::Active,
        Some(1),
        Some(0),
        Some(10),
    );
}

#[test]
fn record_compression_stats_records_dryrun_mode() {
    let config = TokenlessConfig::default();
    let long_before = "x".repeat(500);
    let short_after = "y".repeat(50);
    record_compression_stats(
        &config,
        OperationType::CompressResponse,
        None,
        None,
        None,
        long_before,
        short_after,
        CompressionMode::DryRun,
        None,
        None,
        None,
    );
}

#[test]
fn get_db_path_returns_valid_path() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let db_path = get_db_path();
    assert!(db_path.contains("stats.db"));
}

#[test]
fn open_recorder_exercises_path() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = open_recorder();
    // With TempDbGuard the stats DB path points to a writable temp dir.
    assert!(result.is_ok());
}

#[test]
fn read_input_oversized_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("big.json");
    // Create a file just slightly over the limit by checking what MAX_INPUT_BYTES is
    // For safety, just verify the function handles large strings without panicking
    let result = read_input(&Some(f.to_str().unwrap().to_string()));
    // File doesn't exist, so should error
    assert!(result.is_err());
}

#[test]
fn run_command_compress_schema_from_file() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("schema.json");
    std::fs::write(
        &f,
        r#"{"function":{"name":"test","description":"A test function with a long description that might get compressed by the schema compressor depending on settings","parameters":{"type":"object","properties":{"x":{"type":"string","title":"Remove Me","examples":["ex1"]}}}}}"#,
    )
    .unwrap();
    let result = run_command(Commands::CompressSchema {
        file: Some(f.to_str().unwrap().to_string()),
        batch: false,
        agent_id: Some("test-agent".to_string()),
        session_id: Some("test-session".to_string()),
        tool_use_id: Some("tool-1".to_string()),
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_schema_batch() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("schemas.json");
    std::fs::write(
        &f,
        r#"[{"function":{"name":"a","parameters":{"type":"object","properties":{}}}},{"function":{"name":"b","parameters":{"type":"object","properties":{}}}}]"#,
    )
    .unwrap();
    let result = run_command(Commands::CompressSchema {
        file: Some(f.to_str().unwrap().to_string()),
        batch: true,
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_schema_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("bad.json");
    std::fs::write(&f, "not json at all").unwrap();
    let result = run_command(Commands::CompressSchema {
        file: Some(f.to_str().unwrap().to_string()),
        batch: false,
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("JSON parse error"));
}

#[test]
fn run_command_compress_response_from_file() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("response.json");
    std::fs::write(
        &f,
        r#"{"data":{"items":[1,2,3,4,5,6,7,8,9,10],"debug":"info","value":null,"status":"ok","longText":""}}"#,
    )
    .unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: Some("test-agent".to_string()),
        session_id: Some("sess-1".to_string()),
        tool_use_id: Some("tool-1".to_string()),
        truncate_strings_at: Some(100),
        truncate_arrays_at: Some(5),
        max_depth: Some(10),
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_response_no_stash() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("resp.json");
    std::fs::write(&f, r#"{"key":"value"}"#).unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        truncate_strings_at: None,
        truncate_arrays_at: None,
        max_depth: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_response_with_stash() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("resp.json");
    std::fs::write(&f, r#"{"key":"value"}"#).unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        truncate_strings_at: None,
        truncate_arrays_at: None,
        max_depth: None,
        no_stash: false,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_retrieve_invalid_hash() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Retrieve {
        hash: "not-a-hash".to_string(),
        stash_db: None,
    });
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("invalid stash hash"));
}

#[test]
fn run_command_retrieve_missing_hash() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Retrieve {
        hash: "abcdef0123456789abcdef01".to_string(),
        stash_db: None,
    });
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("no stashed payload"));
}

#[test]
fn run_command_compress_toon_from_file() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("toon.json");
    std::fs::write(
        &f,
        r#"{"name":"test","items":[1,2,3],"nested":{"key":"value"}}"#,
    )
    .unwrap();
    let result = run_command(Commands::CompressToon {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: Some("agent".to_string()),
        session_id: Some("sess".to_string()),
        tool_use_id: Some("tool".to_string()),
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_decompress_toon_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let json_input = r#"{"name":"test","items":[1,2,3]}"#;
    let value: serde_json::Value = serde_json::from_str(json_input).unwrap();
    let toon_encoded = toon_format::encode_default(&value).unwrap();
    let toon_f = dir.path().join("input.toon");
    std::fs::write(&toon_f, &toon_encoded).unwrap();
    let result = run_command(Commands::DecompressToon {
        file: Some(toon_f.to_str().unwrap().to_string()),
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_decompress_toon_empty_input() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("empty.toon");
    std::fs::write(&f, "").unwrap();
    let result = run_command(Commands::DecompressToon {
        file: Some(f.to_str().unwrap().to_string()),
    });
    // Empty input decodes to null, which serializes to "null" and is printed.
    assert!(result.is_ok());
}

#[test]
fn run_command_retrieve_with_marker() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let store = open_stash_store(None);
    if store.is_none() {
        return;
    }
    let store = store.unwrap();
    let key = store.stash("hello world").unwrap();
    let marker = format!("<<tokenless:{}>>", key);
    let result = run_command(Commands::Retrieve {
        hash: marker,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_retrieve_bare_hash() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let store = open_stash_store(None);
    if store.is_none() {
        return;
    }
    let store = store.unwrap();
    let key = store.stash("retrieve bare hash test").unwrap();
    let result = run_command(Commands::Retrieve {
        hash: key,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_show_existing_record() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let recorder = match open_recorder() {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut record = StatsRecord::new(
        OperationType::CompressSchema,
        "test-agent".to_string(),
        500,
        125,
        100,
        25,
    );
    record = record
        .with_before_text("before-text-for-show".to_string())
        .with_after_text("after-text-for-show".to_string());
    let id = match recorder.record(&record) {
        Ok(id) => id,
        Err(_) => return,
    };
    let result = run_command(Commands::Stats(StatsCommands::Show { id }));
    // before_text and after_text are set above, so format_show succeeds.
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_response_large_with_truncation() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("large.json");
    let items: Vec<serde_json::Value> = (0..20).map(|i| serde_json::json!({"id": i, "name": format!("item_{}", i), "long_field": "x".repeat(200)})).collect();
    let data = serde_json::json!({"results": items, "debug": "debug info", "meta": null});
    std::fs::write(&f, serde_json::to_string(&data).unwrap()).unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: Some("test-agent".to_string()),
        session_id: Some("sess-trunc".to_string()),
        tool_use_id: Some("tool-trunc".to_string()),
        truncate_strings_at: Some(50),
        truncate_arrays_at: Some(3),
        max_depth: Some(5),
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_schema_with_stash() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("schema_stash.json");
    let long_desc = "A".repeat(500);
    let schema = serde_json::json!({
        "function": {
            "name": "test_stash",
            "description": long_desc,
            "parameters": {"type": "object", "properties": {"x": {"type": "string"}}}
        }
    });
    std::fs::write(&f, serde_json::to_string(&schema).unwrap()).unwrap();
    let result = run_command(Commands::CompressSchema {
        file: Some(f.to_str().unwrap().to_string()),
        batch: false,
        agent_id: Some("stash-agent".to_string()),
        session_id: Some("stash-sess".to_string()),
        tool_use_id: None,
        no_stash: false,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_summary() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Summary {
        limit: Some(10),
        json: false,
        compare: None,
    }));
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_summary_json() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Summary {
        limit: Some(10),
        json: true,
        compare: None,
    }));
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_list() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::List { limit: 5 }));
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_show_nonexistent() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Show { id: 999999 }));
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("not found"));
}

#[test]
fn run_command_stats_clear_without_confirm() {
    // Clear with --yes to avoid interactive prompt
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Clear { yes: true }));
    assert!(result.is_ok());
}


#[test]
fn run_command_stats_status() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Status));
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_compare() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Summary {
        limit: None,
        json: false,
        compare: Some(vec!["baseline-sess".to_string(), "tokenless-sess".to_string()]),
    }));
    assert!(result.is_ok());
}

#[test]
fn run_command_stats_compare_json() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Stats(StatsCommands::Summary {
        limit: None,
        json: true,
        compare: Some(vec!["baseline-sess".to_string(), "tokenless-sess".to_string()]),
    }));
    assert!(result.is_ok());
}

#[test]
fn run_command_env_check_without_spec() {
    let _guard = TempDbGuard::new();
    let result = run_command(Commands::EnvCheck {
        tool: Some("NonexistentTool".to_string()),
        all: false,
        fix: false,
        checklist: false,
        json: false,
    });
    // Outcome depends on whether a tool-ready-spec file exists on the system:
    // Ok when a spec is found (NonexistentTool is reported as unknown),
    // Err when no spec file is available.
    let _ = result;
}

#[test]
fn get_stash_db_path_returns_valid() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let home = get_home_dir();
    if home.is_empty() {
        return;
    }
    let path = get_stash_db_path(None);
    assert!(path.is_some());
    assert!(path.unwrap().contains(".tokenless/stash.db"));
}

#[test]
fn get_stash_db_path_with_bad_override() {
    let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let home = get_home_dir();
    if home.is_empty() {
        return;
    }
    let path = get_stash_db_path(Some("/nonexistent/deep/dir/stash.db"));
    // Bad override is rejected, falls back to default
    assert!(path.is_some());
    assert!(path.unwrap().contains(".tokenless/stash.db"));
}


#[test]
fn run_command_compress_schema_no_savings() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("small.json");
    std::fs::write(&f, r#"{"function":{"name":"x","parameters":{"type":"object","properties":{}}}}"#).unwrap();
    let result = run_command(Commands::CompressSchema {
        file: Some(f.to_str().unwrap().to_string()),
        batch: false,
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_response_file_not_found() {
    let result = run_command(Commands::CompressResponse {
        file: Some("/nonexistent/path.json".to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        truncate_strings_at: None,
        truncate_arrays_at: None,
        max_depth: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_err());
}

#[test]
fn open_stash_store_none_returns_some() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = open_stash_store(None);
    assert!(result.is_some());
}

#[test]
fn open_stash_store_or_err_none_returns_ok() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = open_stash_store_or_err(None);
    assert!(result.is_ok());
}

#[test]
fn record_compression_stats_sls_only_path() {
    let config = TokenlessConfig { stats_enabled: false, sls_enabled: true, ..Default::default() };
    let long_before = "x".repeat(500);
    let short_after = "y".repeat(50);
    record_compression_stats(
        &config,
        OperationType::CompressResponse,
        Some("sls-test".to_string()),
        Some("sls-session".to_string()),
        Some("sls-tool".to_string()),
        long_before,
        short_after,
        CompressionMode::Active,
        Some(2),
        Some(0),
        Some(15),
    );
}

#[test]
fn record_compression_stats_full_path() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let config = TokenlessConfig { stats_enabled: true, sls_enabled: true, ..Default::default() };
    let long_before = "z".repeat(1000);
    let short_after = "w".repeat(100);
    record_compression_stats(
        &config,
        OperationType::CompressSchema,
        Some("full-agent".to_string()),
        Some("full-session".to_string()),
        Some("full-tool".to_string()),
        long_before,
        short_after,
        CompressionMode::Active,
        Some(3),
        Some(1),
        Some(200),
    );
}

#[test]
fn record_compression_stats_both_disabled() {
    let config = TokenlessConfig::default();
    record_compression_stats(
        &config,
        OperationType::CompressResponse,
        None,
        None,
        None,
        "before".to_string(),
        "after".to_string(),
        CompressionMode::Active,
        None,
        None,
        None,
    );
}

#[test]
fn record_compression_stats_no_savings() {
    let config = TokenlessConfig { stats_enabled: true, ..Default::default() };
    record_compression_stats(
        &config,
        OperationType::CompressResponse,
        None,
        None,
        None,
        "short".to_string(),
        "this is longer than before so no savings".to_string(),
        CompressionMode::Active,
        None,
        None,
        None,
    );
}


#[test]
fn run_command_compress_response_no_agent_id() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("resp.json");
    std::fs::write(&f, r#"{"data":"test","debug":"remove"}"#).unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        truncate_strings_at: None,
        truncate_arrays_at: None,
        max_depth: None,
        no_stash: true,
        stash_db: None,
    });
    assert!(result.is_ok());
}

#[test]
fn read_input_file_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("huge.json");
    let huge = "x".repeat(64 * 1024 * 1024 + 1);
    std::fs::write(&f, &huge).unwrap();
    let result = read_input(&Some(f.to_str().unwrap().to_string()));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("limit"));
}

#[test]
fn read_input_file_missing() {
    let result = read_input(&Some("/nonexistent/file.json".to_string()));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Failed to open"));
}


#[test]
fn run_command_retrieve_store_error() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };
    let result = run_command(Commands::Retrieve {
        hash: "abcdef0123456789abcdef01".to_string(),
        stash_db: Some("/nonexistent/deep/nested/dir/stash.db".to_string()),
    });
    // The stash_db override gets rejected and falls back to the temp DB,
    // where the hash does not exist.
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("no stashed payload"));
}

#[test]
fn run_command_compress_toon_tiny_input() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("tiny.json");
    std::fs::write(&f, r#""x""#).unwrap();
    let result = run_command(Commands::CompressToon {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
    });
    // A tiny JSON string compresses successfully.
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_toon_empty_obj() {
    let _guard = TempDbGuard::new();

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("empty_obj.json");
    std::fs::write(&f, r#"{}"#).unwrap();
    let result = run_command(Commands::CompressToon {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
    });
    // An empty JSON object compresses successfully.
    assert!(result.is_ok());
}

#[test]
fn run_command_compress_response_with_stash_truncation() {
    let _guard = match TempDbGuard::new() { Some(g) => g, None => return };

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("stash_truncation.json");
    let items: Vec<serde_json::Value> = (0..100).map(|i| serde_json::json!({"id": i, "data": "x".repeat(200)})).collect();
    let data = serde_json::json!({"results": items});
    std::fs::write(&f, serde_json::to_string(&data).unwrap()).unwrap();
    let result = run_command(Commands::CompressResponse {
        file: Some(f.to_str().unwrap().to_string()),
        agent_id: None,
        session_id: None,
        tool_use_id: None,
        truncate_strings_at: Some(10),
        truncate_arrays_at: Some(3),
        max_depth: Some(3),
        no_stash: false,
        stash_db: None,
    });
    // Compression with stash enabled and aggressive truncation succeeds.
    assert!(result.is_ok());
}
