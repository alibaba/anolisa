use std::process::Command;

use tokenless_stats::{OperationType, StatsRecord, StatsRecorder, get_home_dir};

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
        if home.is_empty() {
            return None;
        }
        let unique = format!(
            ".tokenless-data-dir-integration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
        );
        let root = std::path::PathBuf::from(home).join(unique);
        std::fs::create_dir_all(&root).ok()?;
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
    assert!(fixture.data_dir.join("stats.db").is_file());

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
    let explicit_dir = fixture.root.join("explicit");
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
    let explicit_dir = fixture.root.join("explicit");
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
    // May fail if no spec file exists, that's OK
    let _ = output.status;
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
