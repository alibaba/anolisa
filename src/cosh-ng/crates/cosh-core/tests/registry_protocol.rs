use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn binary_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("cosh-core");
    path
}

fn run_registry_request(domain: &str, action: &str, params: Value) -> Value {
    let home = tempfile::tempdir().expect("temp home");
    run_registry_request_with_context(domain, action, params, home.path(), None)
}

fn run_registry_request_with_context(
    domain: &str,
    action: &str,
    params: Value,
    home: &Path,
    cwd: Option<&Path>,
) -> Value {
    run_registry_request_with_args(domain, action, params, home, cwd, &[])
}

fn run_registry_request_with_args(
    domain: &str,
    action: &str,
    params: Value,
    home: &Path,
    cwd: Option<&Path>,
    args: &[&str],
) -> Value {
    run_registry_request_with_args_and_env(domain, action, params, home, cwd, args, &[])
}

fn run_registry_request_with_args_and_env(
    domain: &str,
    action: &str,
    params: Value,
    home: &Path,
    cwd: Option<&Path>,
    args: &[&str],
    env: &[(&str, &str)],
) -> Value {
    let bin = binary_path();
    let request = serde_json::json!({
        "type": "registry_request",
        "request_id": "test-1",
        "domain": domain,
        "action": action,
        "params": params,
    });

    let mut command = Command::new(&bin);
    command
        .arg("--registry")
        .args(args)
        .env("HOME", home)
        .env_remove("COSH_AI_PROVIDER")
        .env_remove("COSH_MODEL")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("DASHSCOPE_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ALIBABA_CLOUD_ACCESS_KEY_ID")
        .env_remove("ALIBABA_CLOUD_ACCESS_KEY_SECRET")
        .env_remove("ALIBABA_CLOUD_SECURITY_TOKEN")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn {}: {e}", bin.display()));

    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).unwrap_or_else(|e| panic!("bad JSON: {e}: {l}")))
        .next()
        .expect("expected at least one response line")
}

#[test]
fn bare_registry_reports_env_only_auth_as_satisfied() {
    let home = tempfile::tempdir().expect("temp home");
    let resp = run_registry_request_with_args_and_env(
        "auth",
        "state",
        Value::Null,
        home.path(),
        None,
        &["--bare"],
        &[
            ("COSH_AI_PROVIDER", "gate4"),
            ("COSH_MODEL", "gate4-model"),
            ("OPENAI_BASE_URL", "http://127.0.0.1:1/v1"),
            ("OPENAI_API_KEY", "test-env-only-key"),
        ],
    );

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["saved_providers"], serde_json::json!([]));
    assert_eq!(resp["data"]["effective_auth_required"], false);
}

#[test]
fn bare_registry_does_not_discover_project_skills() {
    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    let skill_dir = project.path().join(".copilot-shell/skills/project-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: project-skill\ndescription: project only\n---\n\nBody.",
    )
    .unwrap();

    let regular = run_registry_request_with_args(
        "skills",
        "list",
        Value::Null,
        home.path(),
        Some(project.path()),
        &[],
    );
    let bare = run_registry_request_with_args(
        "skills",
        "list",
        Value::Null,
        home.path(),
        Some(project.path()),
        &["--bare"],
    );

    assert!(regular["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["name"] == "project-skill"));
    assert!(bare["data"].as_array().unwrap().is_empty(), "{bare}");
}

#[test]
fn registry_extensions_list_returns_success() {
    let resp = run_registry_request("extensions", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_skills_list_returns_success() {
    let resp = run_registry_request("skills", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_hooks_list_returns_success() {
    let resp = run_registry_request("hooks", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["request_id"], "test-1");
    assert_eq!(resp["success"], true);
    assert!(resp["data"].is_array(), "data should be array: {resp}");
}

#[test]
fn registry_auth_state_merges_user_auth_with_project_preferences() {
    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    let home_config_dir = home.path().join(".copilot-shell");
    let project_config_dir = project.path().join(".copilot-shell");
    std::fs::create_dir_all(&home_config_dir).unwrap();
    std::fs::create_dir_all(&project_config_dir).unwrap();
    std::fs::write(
        home_config_dir.join("config.toml"),
        r#"
[ai]
active_provider = "user-dashscope"

[ai.providers.user-dashscope]
type = "dashscope"
api_key = "sk-user"
model = "user-model"
"#,
    )
    .unwrap();
    std::fs::write(
        project_config_dir.join("config.toml"),
        r#"
[ai]
active_provider = "project-provider"
active_model = "project-model"

[ai.providers.project-provider]
type = "dashscope"
api_key = "sk-project"
"#,
    )
    .unwrap();

    let resp = run_registry_request_with_context(
        "auth",
        "state",
        Value::Null,
        home.path(),
        Some(project.path()),
    );
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["active_provider"], "user-dashscope");

    let saved = resp["data"]["saved_providers"].as_array().unwrap();
    assert_eq!(saved.len(), 1, "project provider must be ignored: {resp}");
    assert_eq!(saved[0]["provider_id"], "user-dashscope");
    assert_eq!(saved[0]["api_key_len"], 7);
    assert_eq!(saved[0]["model"], "user-model");
}

#[test]
fn registry_auth_configure_writes_home_config_only() {
    let home = tempfile::tempdir().expect("temp home");
    let project = tempfile::tempdir().expect("temp project");
    let home_config_dir = home.path().join(".copilot-shell");
    let project_config_dir = project.path().join(".copilot-shell");
    std::fs::create_dir_all(&home_config_dir).unwrap();
    std::fs::create_dir_all(&project_config_dir).unwrap();
    let project_config_path = project_config_dir.join("config.toml");
    std::fs::write(
        &project_config_path,
        r#"
[ai]
active_model = "project-model"

[ai.providers.project-provider]
type = "dashscope"
api_key = "sk-project"
"#,
    )
    .unwrap();

    let resp = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "home-provider",
            "provider_type": "dashscope",
            "values": {
                "api_key": "sk-home",
                "model": "home-model"
            }
        }),
        home.path(),
        Some(project.path()),
    );
    assert_eq!(resp["success"], true);

    let home_config = std::fs::read_to_string(home_config_dir.join("config.toml")).unwrap();
    let project_config = std::fs::read_to_string(project_config_path).unwrap();

    assert!(home_config.contains("[ai.providers.home-provider]"));
    assert!(home_config.contains("api_key = \"enc:"));
    assert!(!home_config.contains("sk-home"));
    assert!(!home_config.contains("project-model"));
    assert!(!home_config.contains("project-provider"));
    assert!(project_config.contains("project-model"));
    assert!(project_config.contains("sk-project"));

    let state = run_registry_request_with_context(
        "auth",
        "state",
        Value::Null,
        home.path(),
        Some(project.path()),
    );
    let saved = state["data"]["saved_providers"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["provider_id"], "home-provider");
    assert_eq!(saved[0]["api_key_len"], 7);
}

#[test]
fn registry_auth_configure_encrypts_credentials_starting_with_enc_prefix() {
    let home = tempfile::tempdir().expect("temp home");
    let plaintext = "enc:plaintext-secret";
    let response = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "prefix-provider",
            "provider_type": "dashscope",
            "values": {
                "api_key": plaintext
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(response["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(config.contains("api_key = \"enc:"));
    assert!(!config.contains(plaintext));
    assert!(config_dir.join(".encryption-salt").exists());

    let state = run_registry_request_with_context("auth", "state", Value::Null, home.path(), None);
    let saved = state["data"]["saved_providers"].as_array().unwrap();
    assert_eq!(saved[0]["api_key_len"], plaintext.chars().count());
}

#[test]
fn registry_auth_reconfiguration_clears_opaque_aliyun_credentials() {
    let home = tempfile::tempdir().expect("temp home");
    let configure_static = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "aliyun",
            "provider_type": "aliyun",
            "values": {
                "access_key_id": "test-access-key-id",
                "access_key_secret": "test-access-key-secret",
                "security_token": "test-security-token"
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(configure_static["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    std::fs::write(config_dir.join(".encryption-salt"), [0x33_u8; 32]).unwrap();
    let configure_ecs = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "aliyun",
            "provider_type": "aliyun",
            "values": {
                "auth_source": "ecs_ram_role"
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(configure_ecs["success"], true);

    let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(config.contains("auth_source = \"ecs_ram_role\""));
    assert!(!config.contains("access_key_id"));
    assert!(!config.contains("access_key_secret"));
    assert!(!config.contains("security_token"));
}

#[test]
fn registry_auth_configure_rejects_new_credentials_with_opaque_credentials() {
    let home = tempfile::tempdir().expect("temp home");
    let configure_first = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "first",
            "provider_type": "dashscope",
            "values": {
                "api_key": "first-secret"
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(configure_first["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    let config_path = config_dir.join("config.toml");
    let original = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(config_dir.join(".encryption-salt"), [0x44_u8; 32]).unwrap();

    let configure_second = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "second",
            "provider_type": "dashscope",
            "values": {
                "api_key": "second-secret"
            }
        }),
        home.path(),
        None,
    );

    assert_eq!(configure_second["success"], false);
    assert_eq!(configure_second["error"], "credential_reset_required");
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), original);
}

#[test]
fn registry_auth_reset_recovers_when_multiple_providers_are_opaque() {
    let home = tempfile::tempdir().expect("temp home");
    for (provider_id, api_key) in [("first", "first-secret"), ("second", "second-secret")] {
        let response = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": provider_id,
                "provider_type": "dashscope",
                "values": { "api_key": api_key }
            }),
            home.path(),
            None,
        );
        assert_eq!(response["success"], true);
    }

    let config_dir = home.path().join(".copilot-shell");
    std::fs::write(config_dir.join(".encryption-salt"), [0x55_u8; 32]).unwrap();
    let blocked = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "first",
            "provider_type": "dashscope",
            "values": { "api_key": "replacement-secret" }
        }),
        home.path(),
        None,
    );
    assert_eq!(blocked["success"], false);

    let reset = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "first",
            "provider_type": "dashscope",
            "reset_unavailable_credentials": true,
            "values": { "api_key": "replacement-secret" }
        }),
        home.path(),
        None,
    );
    assert_eq!(reset["success"], true);

    let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(!config.contains("first-secret"));
    assert!(!config.contains("second-secret"));
    assert!(!config.contains("replacement-secret"));
    assert_eq!(config.matches("api_key =").count(), 1);
}

#[test]
fn registry_auth_reset_rotates_invalid_salt() {
    for invalid_salt in [vec![0x11_u8], Vec::new()] {
        let home = tempfile::tempdir().expect("temp home");
        let initial = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": "first",
                "provider_type": "dashscope",
                "values": { "api_key": "first-secret" }
            }),
            home.path(),
            None,
        );
        assert_eq!(initial["success"], true);

        let config_dir = home.path().join(".copilot-shell");
        std::fs::write(config_dir.join(".encryption-salt"), &invalid_salt).unwrap();
        let reset = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": "replacement",
                "provider_type": "dashscope",
                "reset_unavailable_credentials": true,
                "values": { "api_key": "replacement-secret" }
            }),
            home.path(),
            None,
        );

        assert_eq!(reset["success"], true);
        assert_eq!(
            std::fs::read(config_dir.join(".encryption-salt"))
                .unwrap()
                .len(),
            32
        );
        let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert_eq!(config.matches("api_key =").count(), 1);
        assert!(!config.contains("replacement-secret"));
    }
}

#[test]
fn registry_auth_reset_repairs_malformed_salt_without_opaque_credentials() {
    for invalid_salt in [Vec::new(), vec![0x11_u8; 8]] {
        let home = tempfile::tempdir().expect("temp home");
        let config_dir = home.path().join(".copilot-shell");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join(".encryption-salt"), &invalid_salt).unwrap();

        // No encrypted credentials exist yet, so nothing is opaque; the
        // malformed salt still blocks encryption without an explicit reset.
        let blocked = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": "fresh",
                "provider_type": "dashscope",
                "values": { "api_key": "fresh-secret" }
            }),
            home.path(),
            None,
        );
        assert_eq!(blocked["success"], false);
        // The shell only shows its reset confirmation for this exact signal, so
        // a malformed salt must surface as credential_reset_required rather than
        // a generic persistence error the shell cannot act on.
        assert_eq!(blocked["error"], "credential_reset_required");
        assert_eq!(
            std::fs::read(config_dir.join(".encryption-salt")).unwrap(),
            invalid_salt
        );
        assert!(!config_dir.join("config.toml").exists());

        let reset = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": "fresh",
                "provider_type": "dashscope",
                "reset_unavailable_credentials": true,
                "values": { "api_key": "fresh-secret" }
            }),
            home.path(),
            None,
        );
        assert_eq!(reset["success"], true);
        assert_eq!(
            std::fs::read(config_dir.join(".encryption-salt"))
                .unwrap()
                .len(),
            32
        );
        let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
        assert!(config.contains("api_key = \"enc:"));
        assert!(!config.contains("fresh-secret"));
    }
}

#[test]
fn registry_auth_configure_ecs_ram_role_writes_no_salt_or_api_key() {
    let home = tempfile::tempdir().expect("temp home");
    let response = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "aliyun",
            "provider_type": "aliyun",
            "values": { "auth_source": "ecs_ram_role" }
        }),
        home.path(),
        None,
    );
    assert_eq!(response["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    // ECS RAM role has no static credential to encrypt, so no salt is created.
    assert!(!config_dir.join(".encryption-salt").exists());
    let config = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(config.contains("auth_source = \"ecs_ram_role\""));
    // A credential-less provider must not persist an (encrypted) empty api_key.
    assert!(!config.contains("api_key"));
}

#[test]
fn registry_auth_configure_ecs_ram_role_preserves_opaque_peer_without_reset() {
    let home = tempfile::tempdir().expect("temp home");
    let seed = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "peer",
            "provider_type": "dashscope",
            "values": { "api_key": "peer-secret" }
        }),
        home.path(),
        None,
    );
    assert_eq!(seed["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    let config_path = config_dir.join("config.toml");
    let seeded = std::fs::read_to_string(&config_path).unwrap();
    let enc_start = seeded
        .find("api_key = \"")
        .map(|o| o + "api_key = \"".len())
        .unwrap();
    let enc_end = seeded[enc_start..]
        .find('"')
        .map(|o| enc_start + o)
        .unwrap();
    let peer_ciphertext = seeded[enc_start..enc_end].to_string();
    assert!(peer_ciphertext.starts_with("enc:"));

    // Corrupt the salt so the peer's ciphertext can no longer be decrypted.
    std::fs::write(config_dir.join(".encryption-salt"), [0x77_u8; 32]).unwrap();

    // Configuring an ECS RAM role writes no static credential, so it must not
    // require a reset of the unrelated (now opaque) peer credential.
    let ecs = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "aliyun",
            "provider_type": "aliyun",
            "values": { "auth_source": "ecs_ram_role" }
        }),
        home.path(),
        None,
    );
    assert_eq!(
        ecs["success"], true,
        "ECS config must not need a reset: {ecs:?}"
    );

    // The opaque peer ciphertext must survive untouched.
    let after = std::fs::read_to_string(&config_path).unwrap();
    assert!(after.contains(&peer_ciphertext));
    assert!(after.contains("auth_source = \"ecs_ram_role\""));
}

#[test]
fn registry_auth_configure_rejects_empty_credentials_without_touching_state() {
    let home = tempfile::tempdir().expect("temp home");
    let seed = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "seed",
            "provider_type": "dashscope",
            "values": { "api_key": "seed-secret" }
        }),
        home.path(),
        None,
    );
    assert_eq!(seed["success"], true);

    let config_dir = home.path().join(".copilot-shell");
    let config_before = std::fs::read(config_dir.join("config.toml")).unwrap();
    let salt_before = std::fs::read(config_dir.join(".encryption-salt")).unwrap();

    // A blank API key is rejected before any salt rotation or persistence.
    let empty_api_key = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "empty-key",
            "provider_type": "dashscope",
            "values": { "api_key": "   " }
        }),
        home.path(),
        None,
    );
    assert_eq!(empty_api_key["success"], false);

    // Aliyun requires both access key fields when not using an ECS RAM role.
    let empty_aliyun = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "empty-aliyun",
            "provider_type": "aliyun",
            "values": { "access_key_id": "", "access_key_secret": "" }
        }),
        home.path(),
        None,
    );
    assert_eq!(empty_aliyun["success"], false);

    assert_eq!(
        std::fs::read(config_dir.join("config.toml")).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read(config_dir.join(".encryption-salt")).unwrap(),
        salt_before
    );
}

#[test]
fn registry_auth_configure_rejects_unresolvable_masked_secret() {
    let mask = "••••••••";

    // A dashscope api_key whose ciphertext became opaque cannot be re-masked.
    let home = tempfile::tempdir().expect("temp home");
    let config_dir = home.path().join(".copilot-shell");
    let seed = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "dash",
            "provider_type": "dashscope",
            "values": { "api_key": "real-secret" }
        }),
        home.path(),
        None,
    );
    assert_eq!(seed["success"], true);
    std::fs::write(config_dir.join(".encryption-salt"), [0x66_u8; 32]).unwrap();
    let before = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    let rejected = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "dash",
            "provider_type": "dashscope",
            "values": { "api_key": mask }
        }),
        home.path(),
        None,
    );
    assert_eq!(rejected["success"], false);
    // The opaque ciphertext must survive the rejected mask untouched.
    assert_eq!(
        std::fs::read_to_string(config_dir.join("config.toml")).unwrap(),
        before
    );
    assert!(before.contains("api_key = \"enc:"));

    // Each Aliyun secret field fails closed once its ciphertext is unreadable.
    let home = tempfile::tempdir().expect("temp home");
    let config_dir = home.path().join(".copilot-shell");
    let seed = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "aliyun",
            "provider_type": "aliyun",
            "values": {
                "access_key_id": "real-ak",
                "access_key_secret": "real-sk",
                "security_token": "real-token"
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(seed["success"], true);
    std::fs::write(config_dir.join(".encryption-salt"), [0x66_u8; 32]).unwrap();
    let before = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    for field in ["access_key_id", "access_key_secret", "security_token"] {
        let rejected = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": "aliyun",
                "provider_type": "aliyun",
                "values": { field: mask }
            }),
            home.path(),
            None,
        );
        assert_eq!(rejected["success"], false, "field {field} must fail closed");
        assert_eq!(
            std::fs::read_to_string(config_dir.join("config.toml")).unwrap(),
            before
        );
    }
}

#[test]
fn registry_auth_activate_preserves_healthy_credentials_with_tampered_peer() {
    let home = tempfile::tempdir().expect("temp home");
    for (provider_id, api_key) in [("first", "first-secret"), ("second", "second-secret")] {
        let response = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": provider_id,
                "provider_type": "dashscope",
                "values": { "api_key": api_key }
            }),
            home.path(),
            None,
        );
        assert_eq!(response["success"], true);
    }

    let config_path = home.path().join(".copilot-shell/config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    let section_start = config.find("[ai.providers.second]").unwrap();
    let section_end = config[section_start + 1..]
        .find("\n[")
        .map(|offset| section_start + 1 + offset)
        .unwrap_or(config.len());
    let api_start = config[section_start..section_end]
        .find("api_key = \"")
        .map(|offset| section_start + offset + "api_key = \"".len())
        .unwrap();
    let api_end = config[api_start..]
        .find('"')
        .map(|offset| api_start + offset)
        .unwrap();
    let replacement = if config.as_bytes()[api_end - 1] == b'0' {
        '1'
    } else {
        '0'
    };
    let tampered = format!(
        "{}{replacement}{}",
        &config[..api_end - 1],
        &config[api_end..]
    );
    std::fs::write(&config_path, tampered).unwrap();

    let activated = run_registry_request_with_context(
        "auth",
        "activate",
        serde_json::json!({ "provider_id": "first" }),
        home.path(),
        None,
    );
    assert_eq!(activated["success"], true);
}

#[test]
fn registry_auth_metadata_edit_preserves_tampered_peer_without_reset() {
    let home = tempfile::tempdir().expect("temp home");
    for (provider_id, api_key) in [("first", "first-secret"), ("second", "second-secret")] {
        let response = run_registry_request_with_context(
            "auth",
            "configure",
            serde_json::json!({
                "provider_id": provider_id,
                "provider_type": "dashscope",
                "values": { "api_key": api_key }
            }),
            home.path(),
            None,
        );
        assert_eq!(response["success"], true);
    }

    let config_path = home.path().join(".copilot-shell/config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    let section_start = config.find("[ai.providers.second]").unwrap();
    let api_start = config[section_start..]
        .find("api_key = \"")
        .map(|offset| section_start + offset + "api_key = \"".len())
        .unwrap();
    let api_end = config[api_start..]
        .find('"')
        .map(|offset| api_start + offset)
        .unwrap();
    let replacement = if config.as_bytes()[api_end - 1] == b'0' {
        '1'
    } else {
        '0'
    };
    let tampered = format!(
        "{}{replacement}{}",
        &config[..api_end - 1],
        &config[api_end..]
    );
    std::fs::write(&config_path, tampered).unwrap();

    let edited = run_registry_request_with_context(
        "auth",
        "configure",
        serde_json::json!({
            "provider_id": "first",
            "provider_type": "dashscope",
            "values": {
                "api_key": "••••••••••••",
                "model": "qwen3.7-max"
            }
        }),
        home.path(),
        None,
    );
    assert_eq!(edited["success"], true);

    let persisted = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(persisted.matches("api_key =").count(), 2);
    assert!(persisted.contains("model = \"qwen3.7-max\""));
}

#[test]
fn registry_unknown_domain_returns_error() {
    let resp = run_registry_request("unknown_domain", "list", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("unknown domain"));
}

#[test]
fn registry_unsupported_action_returns_error() {
    let resp = run_registry_request("extensions", "invalid_action", Value::Null);
    assert_eq!(resp["type"], "registry_response");
    assert_eq!(resp["success"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("unsupported action"));
}

#[test]
fn registry_extensions_detail_nonexistent_returns_error() {
    let params = serde_json::json!({ "name": "nonexistent-extension-xyz" });
    let resp = run_registry_request("extensions", "detail", params);
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("not found"));
}

#[test]
fn registry_skills_detail_nonexistent_returns_error() {
    let params = serde_json::json!({ "name": "nonexistent-skill-xyz" });
    let resp = run_registry_request("skills", "detail", params);
    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("not found"));
}
