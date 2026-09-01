use std::env;
use std::fs::{self, File};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use uuid::Uuid;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

static BINARIES: OnceLock<Binaries> = OnceLock::new();
static DIRECTORY_NONCE: AtomicU64 = AtomicU64::new(0);

struct Binaries {
    daemon: PathBuf,
    cli: PathBuf,
}

struct TestEnvironment {
    root: PathBuf,
    socket: PathBuf,
    database: PathBuf,
    token: PathBuf,
}

impl TestEnvironment {
    fn new(name: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let nonce = DIRECTORY_NONCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "asc-policy-e2e-{name}-{}-{timestamp}-{nonce}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("E2E runtime directory must be created");
        let environment = Self {
            socket: root.join("daemon.sock"),
            database: root.join("policy.db"),
            token: root.join("policy-admin.token"),
            root,
        };
        environment.prepare_auth();
        environment
    }

    fn prepare_auth(&self) {
        let output = Command::new(&binaries().daemon)
            .arg("prepare-auth")
            .arg("--token-file")
            .arg(&self.token)
            .output()
            .expect("asc-daemon prepare-auth must start");
        assert_process_success("asc-daemon prepare-auth", &output);
    }

    fn start_daemon(&self) -> RunningDaemon {
        RunningDaemon::start(self)
    }

    fn write_json(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.root.join(name);
        fs::write(
            &path,
            serde_json::to_vec_pretty(value).expect("test JSON must serialize"),
        )
        .expect("test JSON file must be written");
        path
    }

    fn write_wrong_token(&self) -> PathBuf {
        let path = self.root.join("wrong-policy-admin.token");
        fs::write(&path, "x".repeat(43)).expect("wrong token fixture must be written");
        path
    }

    fn cli(&self, token: &Path, arguments: &[&str]) -> Output {
        Command::new(&binaries().cli)
            .arg("--socket")
            .arg(&self.socket)
            .arg("--token-file")
            .arg(token)
            .args(arguments)
            .output()
            .expect("asc-cli must start")
    }

    fn cli_ok(&self, arguments: &[&str]) -> Value {
        let output = self.cli(&self.token, arguments);
        assert_process_success("asc-cli", &output);
        serde_json::from_slice(&output.stdout).expect("successful CLI output must be JSON")
    }

    fn cli_error(&self, arguments: &[&str], expected_code: &str) {
        self.cli_error_with_token(&self.token, arguments, expected_code);
    }

    fn cli_error_with_token(&self, token: &Path, arguments: &[&str], expected_code: &str) {
        let output = self.cli(token, arguments);
        assert!(!output.status.success(), "asc-cli unexpectedly succeeded");
        assert!(
            output.stdout.is_empty(),
            "failed CLI command wrote unexpected stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with(&format!("{expected_code}:")),
            "expected error code {expected_code}, got stderr: {stderr}"
        );
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct RunningDaemon {
    child: Option<Child>,
    socket: PathBuf,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl RunningDaemon {
    fn start(environment: &TestEnvironment) -> Self {
        let stdout_log = environment.root.join("daemon.stdout.log");
        let stderr_log = environment.root.join("daemon.stderr.log");
        let stdout = File::create(&stdout_log).expect("daemon stdout log must be created");
        let stderr = File::create(&stderr_log).expect("daemon stderr log must be created");
        let child = Command::new(&binaries().daemon)
            .arg("serve")
            .arg("--socket")
            .arg(&environment.socket)
            .arg("--database")
            .arg(&environment.database)
            .arg("--token-file")
            .arg(&environment.token)
            .env("RUST_LOG", "asc_daemon=warn")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("asc-daemon serve must start");
        let mut daemon = Self {
            child: Some(child),
            socket: environment.socket.clone(),
            stdout_log,
            stderr_log,
        };
        daemon.wait_until_ready();
        daemon
    }

    fn wait_until_ready(&mut self) {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if self.socket.exists() {
                return;
            }
            if let Some(status) = self
                .child
                .as_mut()
                .expect("daemon child must exist while starting")
                .try_wait()
                .expect("daemon status must be readable")
            {
                panic!(
                    "asc-daemon exited before becoming ready with {status}: {}",
                    self.logs()
                );
            }
            assert!(
                Instant::now() < deadline,
                "asc-daemon did not create its socket before the deadline: {}",
                self.logs()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn stop(&mut self) {
        if let Err(problem) = self.stop_inner() {
            panic!("failed to stop asc-daemon: {problem}: {}", self.logs());
        }
    }

    fn stop_inner(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            let signal = Command::new("kill")
                .arg("-TERM")
                .arg(child.id().to_string())
                .status();
            match signal {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("kill -TERM failed with {status}"));
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("could not send SIGTERM: {error}"));
                }
            }
        }
        let status = wait_for_exit(&mut child, STOP_TIMEOUT)?;
        if !status.success() {
            return Err(format!("daemon exited with {status}"));
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        while self.socket.exists() && Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL);
        }
        if self.socket.exists() {
            return Err("owned socket still exists after graceful shutdown".to_owned());
        }
        Ok(())
    }

    fn logs(&self) -> String {
        let stdout = fs::read_to_string(&self.stdout_log).unwrap_or_default();
        let stderr = fs::read_to_string(&self.stderr_log).unwrap_or_default();
        format!("stdout={stdout:?}, stderr={stderr:?}")
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn policy_crud_happy_path_uses_real_daemon_and_cli_processes() {
    let environment = TestEnvironment::new("happy");
    let mut daemon = environment.start_daemon();

    let policy_file = environment.write_json(
        "policy.json",
        &json!({
            "policyName": "protect-e2e-secrets",
            "template": {
                "kind": "high_sensitivity_read_deny",
                "files": ["/e2e/secrets/**"]
            }
        }),
    );
    let created_policy = environment.cli_ok(&[
        "policy",
        "template",
        "put",
        "--file",
        path_text(&policy_file),
    ]);
    assert_eq!(created_policy["disposition"], "STORED");
    assert_eq!(created_policy["policy"]["revision"], 1);
    let policy_id = resource_id(&created_policy["policy"]["policyId"]);
    let fetched_policy =
        environment.cli_ok(&["policy", "template", "get", &policy_id, "--revision", "1"]);
    assert_eq!(fetched_policy, created_policy["policy"]);

    environment.write_json(
        "policy.json",
        &json!({
            "policyId": policy_id,
            "policyName": "prevent-e2e-secret-deletion",
            "template": {
                "kind": "prevent_file_deletion",
                "files": ["/e2e/secrets/**"]
            }
        }),
    );
    let updated_policy = environment.cli_ok(&[
        "policy",
        "template",
        "put",
        "--file",
        path_text(&policy_file),
    ]);
    assert_eq!(updated_policy["policy"]["policyId"], policy_id);
    assert_eq!(updated_policy["policy"]["revision"], 2);
    let policies = environment.cli_ok(&["policy", "template", "list"]);
    assert_eq!(policies["total"], 2);
    assert!(contains_revision(&policies, "policyId", &policy_id, 1));
    assert!(contains_revision(&policies, "policyId", &policy_id, 2));

    let created_scope = environment.cli_ok(&["policy", "scope", "put", "--pid", "4242"]);
    assert_eq!(created_scope["disposition"], "STORED");
    assert_eq!(created_scope["scope"]["revision"], 1);
    let scope_id = resource_id(&created_scope["scope"]["scopeId"]);
    let fetched_scope =
        environment.cli_ok(&["policy", "scope", "get", &scope_id, "--revision", "1"]);
    assert_eq!(fetched_scope, created_scope["scope"]);
    let updated_scope = environment.cli_ok(&[
        "policy",
        "scope",
        "put",
        "--scope-id",
        &scope_id,
        "--cgroup-id",
        "9001",
    ]);
    assert_eq!(updated_scope["scope"]["scopeId"], scope_id);
    assert_eq!(updated_scope["scope"]["revision"], 2);
    let scopes = environment.cli_ok(&["policy", "scope", "list"]);
    assert_eq!(scopes["total"], 2);
    assert!(contains_revision(&scopes, "scopeId", &scope_id, 1));
    assert!(contains_revision(&scopes, "scopeId", &scope_id, 2));

    let created_binding = environment.cli_ok(&[
        "policy",
        "binding",
        "put",
        "--policy-id",
        &policy_id,
        "--policy-revision",
        "1",
        "--scope-id",
        &scope_id,
        "--scope-revision",
        "1",
    ]);
    assert_eq!(created_binding["disposition"], "ACCEPTED");
    assert_eq!(created_binding["binding"]["bindingRevision"], 1);
    assert_eq!(created_binding["binding"]["desiredState"], "READY");
    let binding_id = resource_id(&created_binding["binding"]["bindingId"]);
    let fetched_binding = environment.cli_ok(&["policy", "binding", "get", &binding_id]);
    assert_eq!(fetched_binding, created_binding["binding"]);

    let updated_binding = environment.cli_ok(&[
        "policy",
        "binding",
        "put",
        "--binding-id",
        &binding_id,
        "--policy-id",
        &policy_id,
        "--policy-revision",
        "2",
        "--scope-id",
        &scope_id,
        "--scope-revision",
        "2",
    ]);
    assert_eq!(updated_binding["binding"]["bindingRevision"], 2);
    assert_eq!(updated_binding["binding"]["policy"]["revision"], 2);
    assert_eq!(updated_binding["binding"]["scope"]["revision"], 2);
    let bindings = environment.cli_ok(&["policy", "binding", "list"]);
    assert_eq!(bindings["total"], 1);
    assert_eq!(bindings["items"][0], updated_binding["binding"]);

    daemon.stop();
    assert!(environment.database.exists());
    let mut restarted = environment.start_daemon();
    let persisted = environment.cli_ok(&["policy", "binding", "get", &binding_id]);
    assert_eq!(persisted, updated_binding["binding"]);

    let deleted_binding = environment.cli_ok(&["policy", "binding", "delete", &binding_id]);
    assert_eq!(deleted_binding["disposition"], "ACCEPTED");
    assert_eq!(deleted_binding["binding"]["bindingRevision"], 3);
    assert_eq!(deleted_binding["binding"]["desiredState"], "ABSENT");
    let absent_binding = environment.cli_ok(&["policy", "binding", "get", &binding_id]);
    assert_eq!(absent_binding, deleted_binding["binding"]);

    let deleted_policy = environment.cli_ok(&[
        "policy",
        "template",
        "delete",
        &policy_id,
        "--revision",
        "2",
    ]);
    assert_eq!(deleted_policy["disposition"], "DELETED");
    environment.cli_error(
        &["policy", "template", "get", &policy_id, "--revision", "2"],
        "not_found",
    );
    let retained_policy =
        environment.cli_ok(&["policy", "template", "get", &policy_id, "--revision", "1"]);
    assert_eq!(retained_policy["revision"], 1);

    let deleted_scope =
        environment.cli_ok(&["policy", "scope", "delete", &scope_id, "--revision", "2"]);
    assert_eq!(deleted_scope["disposition"], "DELETED");
    environment.cli_error(
        &["policy", "scope", "get", &scope_id, "--revision", "2"],
        "not_found",
    );
    let retained_scope =
        environment.cli_ok(&["policy", "scope", "get", &scope_id, "--revision", "1"]);
    assert_eq!(retained_scope["revision"], 1);
    assert_eq!(absent_binding["policy"]["revision"], 2);
    assert_eq!(absent_binding["scope"]["revision"], 2);

    restarted.stop();
}

#[test]
#[allow(clippy::too_many_lines)]
fn policy_crud_error_path_is_rejected_without_stopping_the_daemon() {
    let environment = TestEnvironment::new("error");
    let mut daemon = environment.start_daemon();

    let wrong_token = environment.write_wrong_token();
    environment.cli_error_with_token(
        &wrong_token,
        &["policy", "template", "list"],
        "unauthenticated",
    );

    let invalid_policy = environment.write_json(
        "invalid-policy.json",
        &json!({
            "policyName": "invalid-e2e-policy",
            "template": {
                "kind": "high_sensitivity_read_deny",
                "files": ["/e2e/secrets/**"],
                "targetDsl": "must-not-cross-the-CLI-boundary"
            }
        }),
    );
    environment.cli_error(
        &[
            "policy",
            "template",
            "put",
            "--file",
            path_text(&invalid_policy),
        ],
        "invalid_input",
    );

    let unknown_policy_id = Uuid::new_v4().to_string();
    let unknown_policy = environment.write_json(
        "unknown-policy.json",
        &json!({
            "policyId": unknown_policy_id,
            "policyName": "unknown-e2e-policy",
            "template": {
                "kind": "prevent_file_deletion",
                "files": ["/e2e/important/**"]
            }
        }),
    );
    environment.cli_error(
        &[
            "policy",
            "template",
            "put",
            "--file",
            path_text(&unknown_policy),
        ],
        "not_found",
    );

    let policy_file = environment.write_json(
        "valid-policy.json",
        &json!({
            "policyName": "valid-e2e-policy",
            "template": {
                "kind": "prevent_file_deletion",
                "files": ["/e2e/important/**"]
            }
        }),
    );
    let policy = environment.cli_ok(&[
        "policy",
        "template",
        "put",
        "--file",
        path_text(&policy_file),
    ]);
    let policy_id = resource_id(&policy["policy"]["policyId"]);
    environment.cli_error(
        &["policy", "template", "get", &policy_id, "--revision", "99"],
        "not_found",
    );

    let invalid_scope = environment.cli(
        &environment.token,
        &["policy", "scope", "put", "--pid", "0"],
    );
    assert!(!invalid_scope.status.success());
    assert!(String::from_utf8_lossy(&invalid_scope.stderr).contains("must be greater than zero"));

    let unknown_scope_id = Uuid::new_v4().to_string();
    environment.cli_error(
        &[
            "policy",
            "scope",
            "put",
            "--scope-id",
            &unknown_scope_id,
            "--pid",
            "4242",
        ],
        "not_found",
    );
    let scope = environment.cli_ok(&["policy", "scope", "put", "--pid", "4242"]);
    let scope_id = resource_id(&scope["scope"]["scopeId"]);

    environment.cli_error(
        &[
            "policy",
            "binding",
            "put",
            "--policy-id",
            &unknown_policy_id,
            "--policy-revision",
            "1",
            "--scope-id",
            &scope_id,
            "--scope-revision",
            "1",
        ],
        "not_found",
    );
    environment.cli_error(
        &[
            "policy",
            "binding",
            "put",
            "--policy-id",
            &policy_id,
            "--policy-revision",
            "1",
            "--scope-id",
            &unknown_scope_id,
            "--scope-revision",
            "1",
        ],
        "not_found",
    );
    let unknown_binding_id = Uuid::new_v4().to_string();
    environment.cli_error(
        &["policy", "binding", "delete", &unknown_binding_id],
        "not_found",
    );

    let policies = environment.cli_ok(&["policy", "template", "list"]);
    assert_eq!(policies["total"], 1);
    assert_eq!(policies["items"][0]["policyId"], policy_id);
    let scopes = environment.cli_ok(&["policy", "scope", "list"]);
    assert_eq!(scopes["total"], 1);
    assert_eq!(scopes["items"][0]["scopeId"], scope_id);

    daemon.stop();
}

fn binaries() -> &'static Binaries {
    BINARIES.get_or_init(build_binaries)
}

fn build_binaries() -> Binaries {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root must resolve");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(&workspace)
        .arg("build")
        .arg("--quiet")
        .args(["-p", "asc-daemon", "--bin", "asc-daemon"])
        .args(["-p", "asc-cli", "--bin", "asc-cli"]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let output = command.output().expect("cargo build must start");
    assert_process_success("cargo build for process E2E", &output);

    let target_root = env::var_os("CARGO_TARGET_DIR").map_or_else(
        || workspace.join("target"),
        |value| absolute_from(&workspace, Path::new(&value)),
    );
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let executable_suffix = env::consts::EXE_SUFFIX;
    let daemon = target_root
        .join(profile)
        .join(format!("asc-daemon{executable_suffix}"));
    let cli = target_root
        .join(profile)
        .join(format!("asc-cli{executable_suffix}"));
    assert!(
        daemon.is_file(),
        "asc-daemon binary missing at {}",
        daemon.display()
    );
    assert!(cli.is_file(), "asc-cli binary missing at {}", cli.display());
    Binaries { daemon, cli }
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("daemon did not stop before the deadline".to_owned());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn assert_process_success(name: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{name} failed with {}: stdout={}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn resource_id(value: &Value) -> String {
    let value = value.as_str().expect("resource id must be a string");
    Uuid::parse_str(value).expect("daemon-owned resource id must be a UUID");
    value.to_owned()
}

fn contains_revision(page: &Value, id_field: &str, id: &str, revision: u64) -> bool {
    page["items"]
        .as_array()
        .expect("list items must be an array")
        .iter()
        .any(|item| item[id_field] == id && item["revision"] == revision)
}

fn path_text(path: &Path) -> &str {
    path.as_os_str()
        .to_str()
        .unwrap_or_else(|| panic!("test path is not UTF-8: {}", path.display()))
}
