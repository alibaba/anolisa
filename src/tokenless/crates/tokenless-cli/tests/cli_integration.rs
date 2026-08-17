use std::process::Command;

use tokenless_ccr::StashStore;
use tokenless_runtime::{CompressOptions, compress_response_with_store};
use tokenless_stats::{OperationType, StatsRecord, StatsRecorder, estimate_tokens, get_home_dir};

fn tokenless_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tokenless"))
}

struct TempStatsDb {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
    record_id: i64,
}

impl TempStatsDb {
    fn new() -> Option<Self> {
        let home = get_home_dir();
        if home.is_empty() {
            return None;
        }
        let unique = format!(
            ".tokenless-cli-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
        );
        let dir = std::path::PathBuf::from(home).join(unique);
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("stats.db");
        let recorder = StatsRecorder::new(&path).ok()?;
        let record_id = recorder
            .record(
                &StatsRecord::new(
                    OperationType::CompressResponse,
                    "integration-agent".to_string(),
                    17,
                    10,
                    9,
                    5,
                )
                .with_session_id("integration-session")
                .with_tool_use_id("integration-tool")
                .with_text("keep\nremove\n".to_string(), "keep\n".to_string()),
            )
            .ok()?;
        Some(Self {
            dir,
            path,
            record_id,
        })
    }

    fn command(&self) -> Command {
        let mut command = tokenless_bin();
        command.env("TOKENLESS_STATS_DB", &self.path);
        command
    }
}

impl Drop for TempStatsDb {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

struct TempDataDir {
    root: std::path::PathBuf,
    data_dir: std::path::PathBuf,
}

impl TempDataDir {
    fn new() -> Option<Self> {
        let home = get_home_dir();
        let unique = format!(
            "tokenless-external-data-dir-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).ok()?;
        if !home.is_empty()
            && root
                .canonicalize()
                .ok()?
                .starts_with(std::path::Path::new(&home).canonicalize().ok()?)
        {
            std::fs::remove_dir_all(&root).ok();
            return None;
        }
        let data_dir = root.join("databases");
        Some(Self { root, data_dir })
    }

    fn command(&self) -> Command {
        let mut command = tokenless_bin();
        command
            .env("TOKENLESS_DATA_DIR", &self.data_dir)
            .env_remove("TOKENLESS_STATS_DB")
            .env_remove("TOKENLESS_STASH_DB");
        command
    }
}

impl Drop for TempDataDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn data_dir_env_routes_stats_and_stash_databases() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };

    let stats_output = fixture
        .command()
        .args(["stats", "summary"])
        .output()
        .unwrap();
    assert!(
        stats_output.status.success(),
        "stats command failed: {}",
        String::from_utf8_lossy(&stats_output.stderr)
    );
    assert!(!String::from_utf8_lossy(&stats_output.stderr).contains("ignoring TOKENLESS_DATA_DIR"));
    assert!(fixture.data_dir.join("stats.db").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fixture.data_dir.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    let stash_output = fixture
        .command()
        .args(["retrieve", "abcdef0123456789abcdef01"])
        .output()
        .unwrap();
    assert!(!stash_output.status.success());
    assert!(fixture.data_dir.join("stash.db").is_file());
}

#[test]
fn stats_db_env_takes_precedence_over_data_dir() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let explicit_dir = fixture.data_dir.join("explicit");
    std::fs::create_dir_all(&explicit_dir).unwrap();
    let explicit_db = explicit_dir.join("stats.db");

    let output = fixture
        .command()
        .env("TOKENLESS_STATS_DB", &explicit_db)
        .args(["stats", "summary"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stats command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(explicit_db.is_file());
    assert!(!fixture.data_dir.join("stats.db").exists());
}

#[test]
fn stash_db_env_takes_precedence_over_data_dir() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let explicit_dir = fixture.data_dir.join("explicit");
    std::fs::create_dir_all(&explicit_dir).unwrap();
    let explicit_db = explicit_dir.join("stash.db");

    let output = fixture
        .command()
        .env("TOKENLESS_STASH_DB", &explicit_db)
        .args(["retrieve", "abcdef0123456789abcdef01"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(explicit_db.is_file());
    assert!(!fixture.data_dir.join("stash.db").exists());
}

#[test]
fn invalid_explicit_data_dir_does_not_fall_back_to_home() {
    let output = tokenless_bin()
        .env("TOKENLESS_DATA_DIR", "relative/data")
        .env_remove("TOKENLESS_STATS_DB")
        .args(["stats", "summary"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("path 'relative/data' is not absolute"));
}

#[test]
fn invalid_explicit_data_dir_does_not_block_stats_status() {
    let output = tokenless_bin()
        .env("TOKENLESS_DATA_DIR", "relative/data")
        .env_remove("TOKENLESS_STATS_DB")
        .args(["stats", "status"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Stats recording:"));
    assert!(output.stderr.is_empty());
}

#[test]
fn valid_stats_db_override_wins_over_invalid_data_dir() {
    let fixture = match TempStatsDb::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let output = fixture
        .command()
        .env("TOKENLESS_DATA_DIR", "relative/data")
        .args(["stats", "summary"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(fixture.path.is_file());
}

#[test]
fn stats_db_override_cannot_escape_selected_data_dir() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let outside_db = fixture.root.join("outside-stats.db");
    let output = fixture
        .command()
        .env("TOKENLESS_STATS_DB", &outside_db)
        .args(["stats", "summary"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!outside_db.exists());
    assert!(fixture.data_dir.join("stats.db").is_file());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ignoring TOKENLESS_STATS_DB"));
}

#[test]
fn compress_schema_from_stdin() {
    let schema = r#"{"function":{"name":"test","description":"A test function","parameters":{"type":"object","properties":{"x":{"type":"string","title":"Remove Me","examples":["ex1"]}}}}}"#;
    let output = tokenless_bin()
        .args(["compress-schema"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schema.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success(), "compress-schema should succeed");
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("output should be valid JSON");
    assert!(result["function"]["name"].is_string());
}

#[test]
fn compress_schema_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("schema.json");
    std::fs::write(
        &f,
        r#"{"function":{"name":"f","description":"desc","parameters":{"type":"object","properties":{}}}}"#,
    )
    .unwrap();
    let output = tokenless_bin()
        .args(["compress-schema", "--file", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["function"]["name"], "f");
}

#[test]
fn compress_schema_batch_mode() {
    let schemas = r#"[{"function":{"name":"a","parameters":{"type":"object","properties":{}}}},{"function":{"name":"b","parameters":{"type":"object","properties":{}}}}]"#;
    let output = tokenless_bin()
        .args(["compress-schema", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schemas.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.is_array());
}

#[test]
fn compress_schema_batch_gemini_function_declarations() {
    // copilot-shell BeforeModel hooks pass Gemini SDK tool entries
    // ({"functionDeclarations": [...]}). The batch path must compress the
    // nested declarations and keep the wrapper shape so the host can apply
    // the rewritten array unchanged.
    let long_desc = "Run a shell command in the workspace. ".repeat(20);
    let schemas = serde_json::json!([
        {
            "functionDeclarations": [
                {
                    "name": "shell",
                    "description": long_desc,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {"type": "string", "description": long_desc}
                        }
                    }
                }
            ]
        }
    ]);
    let input = serde_json::to_string(&schemas).unwrap();
    let output = tokenless_bin()
        .args(["compress-schema", "--batch"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success(), "compress-schema should succeed");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let decls = &result[0]["functionDeclarations"];
    assert_eq!(decls[0]["name"], "shell");
    // Limits are char counts, not byte lengths (the stash marker carries a
    // multibyte ellipsis).
    assert!(decls[0]["description"].as_str().unwrap().chars().count() <= 256);
    let param_desc = decls[0]["parameters"]["properties"]["command"]["description"]
        .as_str()
        .unwrap();
    assert!(param_desc.chars().count() <= 160);
    assert!(
        output.stdout.len() < input.len(),
        "compressed output must be smaller than the input"
    );
}

#[test]
fn compress_response_from_stdin() {
    let response =
        r#"{"data":"value","debug":"remove","trace":"remove","empty_field":"","null_field":null}"#;
    let output = tokenless_bin()
        .args(["compress-response"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.get("data").is_some());
    assert!(result.get("debug").is_none());
}

#[test]
fn compress_response_cli_matches_runtime_library() {
    let response = serde_json::to_string(&serde_json::json!({
        "items": (0..100).collect::<Vec<_>>(),
        "debug": "remove",
        "empty": null,
    }))
    .unwrap();
    let expected = compress_response_with_store(
        &response,
        &CompressOptions {
            truncate_arrays_at: Some(4),
            stash_enabled: false,
            ..CompressOptions::default()
        },
        true,
        None,
    )
    .unwrap();

    let output = tokenless_bin()
        .env("TOKENLESS_COMPRESSION_ENABLED", "1")
        .env("TOKENLESS_STATS_ENABLED", "0")
        .env("TOKENLESS_SLS_ENABLED", "0")
        .args([
            "compress-response",
            "--truncate-arrays-at",
            "4",
            "--no-stash",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim_end(),
        expected.output,
    );
}

#[test]
fn compress_response_stats_use_unicode_aware_estimates() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let response = serde_json::to_string(&serde_json::json!({
        "tail": "世界".repeat(300)
    }))
    .unwrap();
    let output = fixture
        .command()
        .env("TOKENLESS_COMPRESSION_ENABLED", "1")
        .env("TOKENLESS_STATS_ENABLED", "1")
        .env("TOKENLESS_SLS_ENABLED", "0")
        .args([
            "compress-response",
            "--truncate-strings-at",
            "80",
            "--no-stash",
            "--agent-id",
            "integration-agent",
            "--session-id",
            "unicode-session",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "compress-response failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let compressed = String::from_utf8(output.stdout).unwrap();
    let recorder = StatsRecorder::new(fixture.data_dir.join("stats.db")).unwrap();
    let records = recorder
        .records_by_session("unicode-session", None)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].before_tokens, estimate_tokens(&response));
    assert_eq!(
        records[0].after_tokens,
        estimate_tokens(compressed.trim_end())
    );
}

#[test]
fn dry_run_no_savings_keeps_the_no_savings_warning() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let response = r#"{"value":1}"#;
    let output = fixture
        .command()
        .env("TOKENLESS_COMPRESSION_ENABLED", "0")
        .env("TOKENLESS_STATS_ENABLED", "0")
        .env("TOKENLESS_SLS_ENABLED", "0")
        .args(["compress-response", "--no-stash"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim_end(),
        response
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("response compression did not reduce size"));
    assert!(stderr.contains("dry-run mode"));
}

#[test]
fn compress_response_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("response.json");
    std::fs::write(&f, r#"{"key":"value","logs":"remove me"}"#).unwrap();
    let output = tokenless_bin()
        .args(["compress-response", "--file", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.get("key").is_some());
}

#[test]
fn compress_response_no_stash() {
    let response = r#"{"data":"value","debug":"remove"}"#;
    let output = tokenless_bin()
        .args(["compress-response", "--no-stash"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn stats_list_empty() {
    let output = tokenless_bin().args(["stats", "list"]).output().unwrap();
    // May succeed or fail depending on db state; should not panic
    let _ = output.status;
}

#[test]
fn stats_summary() {
    let output = tokenless_bin().args(["stats", "summary"]).output().unwrap();
    let _ = output.status;
}

#[test]
fn retrieve_missing_hash() {
    let output = tokenless_bin()
        .args(["retrieve", "000000000000000000000000"])
        .output()
        .unwrap();
    // Should fail gracefully (hash not found), not panic
    assert!(!output.status.success());
}

#[test]
fn retrieve_invalid_hash() {
    let output = tokenless_bin()
        .args(["retrieve", "not-a-valid-hash"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn retrieve_stdout_is_byte_exact_without_extra_trailing_newline() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let stash_db = fixture.data_dir.join("stash.db");
    // Long string forces reversible truncation + stash. The stored payload is
    // this string value (not the whole JSON document) and has no trailing `\n`.
    let stashed_string = format!("HELLO_RETRIEVE_EXACT_{}", "X".repeat(200));
    let original = format!("{{\"s\":\"{stashed_string}\"}}");

    let compressed = fixture
        .command()
        // Force compression on so a caller/home config with
        // TOKENLESS_COMPRESSION_ENABLED=0 (or compression_enabled:false)
        // cannot dry-run this subprocess and skip the stash marker.
        .env("TOKENLESS_COMPRESSION_ENABLED", "1")
        .env("TOKENLESS_STATS_ENABLED", "0")
        .args([
            "compress-response",
            "--truncate-strings-at",
            "80",
            "--stash-db",
        ])
        .arg(&stash_db)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(original.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        compressed.status.success(),
        "compress-response failed: {}",
        String::from_utf8_lossy(&compressed.stderr)
    );
    let compressed_text = String::from_utf8_lossy(&compressed.stdout);
    let marker_start = compressed_text
        .find("<<tokenless:")
        .expect("compressed output should contain a stash marker");
    let marker_end = compressed_text[marker_start..]
        .find(">>")
        .map(|i| marker_start + i + 2)
        .expect("stash marker should be closed");
    let marker = &compressed_text[marker_start..marker_end];

    let retrieved = fixture
        .command()
        .args(["retrieve", marker, "--stash-db"])
        .arg(&stash_db)
        .output()
        .unwrap();
    assert!(
        retrieved.status.success(),
        "retrieve failed: {}",
        String::from_utf8_lossy(&retrieved.stderr)
    );
    assert_eq!(
        retrieved.stdout.as_slice(),
        stashed_string.as_bytes(),
        "retrieve must restore the stashed payload byte-for-byte; \
         an extra trailing newline breaks end-to-end lossless recovery"
    );
}

#[test]
fn compress_response_no_savings_rolls_back_orphan_stash() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    let stash_db = fixture.data_dir.join("stash.db");
    // Small array + aggressive truncate: marker overhead makes after_tokens
    // >= before_tokens, so CLI falls back to the original input.
    let original = r#"["a","b"]"#;

    let output = fixture
        .command()
        .env("TOKENLESS_COMPRESSION_ENABLED", "1")
        .env("TOKENLESS_STATS_ENABLED", "0")
        .args([
            "compress-response",
            "--truncate-arrays-at",
            "1",
            "--stash-db",
        ])
        .arg(&stash_db)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(original.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "compress-response failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("did not reduce size"),
        "expected no-savings path, stderr={stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("tokenless:"),
        "no-savings stdout must not expose markers: {stdout}"
    );
    // Discarded markers must not leave orphan rows in stash.db.
    let live = if stash_db.exists() {
        tokenless_ccr::SqliteStore::new(&stash_db)
            .map(|s| s.len())
            .unwrap_or(0)
    } else {
        0
    };
    assert_eq!(
        live,
        0,
        "no-savings discard must roll back stash writes; orphan rows remain in {}",
        stash_db.display()
    );
}

#[test]
fn compress_schema_no_savings_rolls_back_orphan_stash() {
    let fixture = match TempDataDir::new() {
        Some(fixture) => fixture,
        None => return,
    };
    // Description just over the default 256-char cap: truncation + stash
    // marker often yields after_tokens >= before_tokens (1-char savings
    // does not always change the estimate). Pad the function name so the
    // compact JSON length hits that equality.
    let mut hit_no_savings = false;
    for name in ["x", "xx", "xxx", "xxxx"] {
        // Each candidate owns its database so a prior savings-path marker
        // cannot make this candidate's orphan-row assertion fail.
        let stash_db = fixture.data_dir.join(format!("stash-{name}.db"));
        let schema = serde_json::json!({
            "function": {
                "name": name,
                "description": "A".repeat(257),
                "parameters": {"type": "object", "properties": {}}
            }
        });
        let original = serde_json::to_string(&schema).unwrap();
        let output = fixture
            .command()
            .env("TOKENLESS_COMPRESSION_ENABLED", "1")
            .env("TOKENLESS_STATS_ENABLED", "0")
            .args(["compress-schema", "--stash-db"])
            .arg(&stash_db)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(original.as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(
            output.status.success(),
            "compress-schema failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("did not reduce size") {
            continue;
        }
        hit_no_savings = true;
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("tokenless:"),
            "no-savings stdout must not expose markers: {stdout}"
        );
        let live = if stash_db.exists() {
            tokenless_ccr::SqliteStore::new(&stash_db)
                .map(|s| s.len())
                .unwrap_or(0)
        } else {
            0
        };
        assert_eq!(
            live,
            0,
            "no-savings discard must roll back stash writes; orphan rows remain in {}",
            stash_db.display()
        );
        break;
    }
    assert!(
        hit_no_savings,
        "failed to hit compress-schema no-savings path with a just-over-limit description"
    );
}

#[test]
fn no_args_shows_error() {
    let output = tokenless_bin().output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn invalid_json_input() {
    let output = tokenless_bin()
        .args(["compress-schema"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"not valid json")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn compress_schema_with_agent_id() {
    let schema = r#"{"function":{"name":"test","parameters":{"type":"object","properties":{}}}}"#;
    let output = tokenless_bin()
        .args(["compress-schema", "--agent-id", "test-agent"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(schema.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn compress_response_with_session_and_tool_ids() {
    let response = r#"{"data":"value"}"#;
    let output = tokenless_bin()
        .args([
            "compress-response",
            "--agent-id",
            "test",
            "--session-id",
            "s1",
            "--tool-use-id",
            "t1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(response.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn compress_toon_from_stdin() {
    let toon_input = r#"{"content":"some content","debug":"remove"}"#;
    let output = tokenless_bin()
        .args(["compress-toon"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(toon_input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    // compress-toon may or may not succeed depending on input format
    let _ = output.status;
}

#[test]
fn env_check_without_spec() {
    let output = tokenless_bin().args(["env-check"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Specify --tool <name> or --all"));
}

#[test]
fn env_check_is_hard_bypassed_even_with_legacy_opt_in() {
    let output = tokenless_bin()
        .args(["env-check", "--tool", "Shell", "--json"])
        .env("TOKENLESS_TOOL_READY_ENABLED", "1")
        .env(
            "TOKENLESS_TOOL_READY_SPEC",
            "/path/that/must/not/be-read-while-disabled",
        )
        .output()
        .unwrap();

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result.as_object().unwrap().len(), 3);
    assert_eq!(result["tool"], "Shell");
    assert_eq!(result["status"], "UNKNOWN");
    assert_eq!(result["enabled"], false);
}

#[test]
fn config_show() {
    let output = tokenless_bin().args(["config", "show"]).output().unwrap();
    // Should show current config or defaults
    let _ = output.status;
}

#[test]
fn stats_show_single_nonexistent() {
    let output = tokenless_bin()
        .args(["stats", "show", "99999"])
        .output()
        .unwrap();
    // Should fail gracefully for nonexistent record
    let _ = output.status;
}

#[test]
fn stats_diff_record_json_contains_structured_hunks() {
    let db = match TempStatsDb::new() {
        Some(db) => db,
        None => return,
    };
    let output = db
        .command()
        .args(["stats", "diff", &db.record_id.to_string(), "--json"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], "1.0");
    assert_eq!(json["scope"]["kind"], "record");
    assert_eq!(json["chains"][0]["diff"]["available"], true);
    assert!(json["chains"][0]["diff"]["hunks"].is_array());
}

#[test]
fn stats_diff_session_omits_content_hunks() {
    let db = match TempStatsDb::new() {
        Some(db) => db,
        None => return,
    };
    let output = db
        .command()
        .args([
            "stats",
            "diff",
            "--session",
            "integration-session",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["scope"]["kind"], "session");
    assert!(json["chains"][0].get("diff").is_none());
}

#[test]
fn stats_diff_tool_use_renders_terminal_diff() {
    let db = match TempStatsDb::new() {
        Some(db) => db,
        None => return,
    };
    let output = db
        .command()
        .args([
            "stats",
            "diff",
            "--session",
            "integration-session",
            "--tool-use-id",
            "integration-tool",
            "--no-color",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Estimated tokens: 10 -> 5"));
    assert!(stdout.contains("-remove"));
    assert!(!stdout.contains("\u{1b}["));
}

#[test]
fn stats_diff_invalid_scope_and_missing_record_use_expected_exit_codes() {
    let invalid = tokenless_bin()
        .args(["stats", "diff", "42", "--session", "session"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));

    let db = match TempStatsDb::new() {
        Some(db) => db,
        None => return,
    };
    let missing = db
        .command()
        .args(["stats", "diff", "999999"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
}

fn write_checklist_spec(dir: &std::path::Path) -> std::path::PathBuf {
    // Config file that reliably exists for the config-file check.
    let config_path = dir.join("present.conf");
    std::fs::write(&config_path, "").unwrap();
    let spec = serde_json::json!({
        "_comment": "comment keys must be skipped",
        "Write": {
            "required": ["nonexistent_binary_xyz_98"],
            "recommended": [],
            "config_files": [],
            "permissions": [],
            "network": []
        },
        "Shell": {
            "required": ["bash"],
            "recommended": [],
            "config_files": [],
            "permissions": [],
            "network": []
        },
        "WebFetch": {
            "required": [],
            "recommended": ["nonexistent_binary_xyz_99"],
            "config_files": [],
            "permissions": [],
            "network": ["lan_probe"]
        },
        "Read": {
            "required": ["bash"],
            "recommended": [],
            "config_files": [config_path],
            "permissions": ["exec_shell"],
            "network": []
        }
    });
    let spec_path = dir.join("checklist-spec.json");
    std::fs::write(&spec_path, spec.to_string()).unwrap();
    spec_path
}

#[test]
fn env_check_checklist_json_is_hard_bypassed() {
    let dir = tempfile::tempdir().unwrap();
    let spec_path = write_checklist_spec(dir.path());

    let output = tokenless_bin()
        .args(["env-check", "--checklist", "--json"])
        .env("TOKENLESS_TOOL_READY_ENABLED", "1")
        .env("TOKENLESS_TOOL_READY_SPEC", &spec_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "env-check --checklist --json should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("--checklist --json must print one JSON object on stdout");
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert_eq!(value["tool"], "checklist");
    assert_eq!(value["status"], "UNKNOWN");
    assert_eq!(value["enabled"], false);
    assert!(value.get("tools").is_none());
    assert!(value.get("summary").is_none());
}

#[test]
fn env_check_hard_bypass_json_is_stable_across_processes() {
    let dir = tempfile::tempdir().unwrap();
    let spec_path = write_checklist_spec(dir.path());

    let mut outputs = Vec::new();
    for _ in 0..8 {
        let output = tokenless_bin()
            .args(["env-check", "--checklist", "--json"])
            .env("TOKENLESS_TOOL_READY_ENABLED", "1")
            .env("TOKENLESS_TOOL_READY_SPEC", &spec_path)
            .output()
            .unwrap();
        assert!(output.status.success());
        outputs.push(output.stdout);
    }

    for (index, stdout) in outputs.iter().enumerate().skip(1) {
        assert_eq!(
            stdout,
            &outputs[0],
            "hard-bypass JSON must be byte-identical across processes (run {})",
            index + 1
        );
    }
}
