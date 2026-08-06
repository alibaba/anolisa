//! End-to-end coverage for the adapter actions a component update leaves
//! behind (issue #1885): human output, `--quiet`, `--json`, and `--dry-run`.
//!
//! The fixture installs a raw component whose adapter bundle is covered by
//! an enabled receipt, then publishes a newer artifact with a changed
//! bundle. Running the real binary through `update` must name the stale
//! adapter and its exact recovery command without touching the receipt.

use std::path::{Path, PathBuf};
use std::process::Output;

use anolisa_core::adapter::claim::{
    AdapterClaim, CLAIM_SCHEMA_VERSION, ClaimStatus, CoshClaim, DRIVER_SCHEMA_VERSION,
    DriverPayload,
};
use anolisa_core::state_store::StateStore;
use anolisa_core::{
    FileOwner, InstallMode as StateInstallMode, InstalledObject, InstalledState, ObjectKind,
    ObjectStatus, OwnedFile, OwnedFileKind, Ownership, SubscriptionScope,
};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::privilege;

mod common;

const COMPONENT: &str = "stale-demo";

fn receipt_digest(root: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};

    fn collect_files(dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect_files(&path, files)?;
            } else {
                files.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect_files(root, &mut files).ok()?;
    files.sort();
    let mut hasher = Sha256::new();
    for path in files {
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let bytes = std::fs::read(&path).ok()?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    Some(format!("sha256:{:x}", hasher.finalize()))
}

fn manifest(version: &str, with_adapter: bool) -> String {
    manifest_with_hook(version, with_adapter, HookFixture::None)
}

/// A `post_install` hook that always fails, for the rollback path.
const FAILING_HOOK: &[u8] = br#"#!/bin/sh
echo "sweep failed" >&2
exit 1
"#;

/// What the incoming v0.2.0 contract declares for `post_install`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HookFixture {
    /// No hook at all.
    None,
    /// A hook that sweeps the bundle and records that it ran.
    Sweeping,
    /// A hook that exits non-zero.
    Failing,
    /// A contract authoring bug: the script path uses an unknown placeholder,
    /// so it can never resolve and no script is shipped.
    InvalidPath,
}

/// A non-`None` `hook` ships a `post_install` declaration, so the update
/// path's hook slot can be observed end to end.
fn manifest_with_hook(version: &str, with_adapter: bool, hook: HookFixture) -> String {
    let with_hook = hook != HookFixture::None;
    let script = if hook == HookFixture::InvalidPath {
        "{no_such_placeholder}/post-install.sh"
    } else {
        "{datadir}/hooks/{component}/post-install.sh"
    };
    let adapter = if with_adapter {
        format!(
            r#"
[[adapters]]
framework = "openclaw"
adapter_type = "plugin"
source = "adapters/{COMPONENT}/openclaw"
dest = "{{datadir}}/adapters/{{component}}/openclaw/"
"#
        )
    } else {
        String::new()
    };
    let hook_toml = if !with_hook {
        String::new()
    } else if hook == HookFixture::InvalidPath {
        format!(
            r#"
[[component.hooks]]
phase = "post_install"
script = "{script}"
strict = true
"#
        )
    } else {
        format!(
            r#"
[[component.layout.files]]
source = "share/anolisa/hooks/{COMPONENT}/post-install.sh"
target = "{script}"
mode = "0755"
type = "executable"

[[component.hooks]]
phase = "post_install"
script = "{script}"
strict = true
"#
        )
    };
    format!(
        r#"[component]
name = "{COMPONENT}"
version = "{version}"

[component.layout]
modes = ["system", "user"]

[[component.layout.files]]
source = "bin/{COMPONENT}"
target = "{{bindir}}/{COMPONENT}"
mode = "0755"
type = "executable"
{hook_toml}{adapter}"#
    )
}

/// Sweeps `__pycache__` from the adapter bundle and records that it ran.
/// Derives the datadir from its own location, the way a component-shipped
/// hook must: nothing is passed on the argv.
const POST_INSTALL_HOOK: &[u8] = br#"#!/bin/sh
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
datadir="$(cd "$here/../.." && pwd)"
find "$datadir/adapters/stale-demo/openclaw" -type d -name __pycache__ -prune \
    -exec rm -rf {} +
printf 'ran\n' > "$here/ran.marker"
"#;

fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(enc);
    for (path, data) in entries {
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, *path, *data)
            .expect("append tar entry");
    }
    tar.into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

struct UpdateFixture {
    _tmp: tempfile::TempDir,
    system_prefix: PathBuf,
    home: PathBuf,
    data_home: PathBuf,
    config_home: PathBuf,
    state_home: PathBuf,
    cache_home: PathBuf,
    runtime_dir: PathBuf,
    user_layout: FsLayout,
    bundle_root: PathBuf,
    enable_digest: String,
}

impl UpdateFixture {
    fn new() -> Self {
        Self::build(HookFixture::None)
    }

    /// Fixture whose *incoming* v0.2.0 contract declares a strict
    /// `post_install` hook. The installed v0.1.0 snapshot does not, so this
    /// also covers an upgrade that adds the hook.
    fn with_post_install_hook() -> Self {
        Self::build(HookFixture::Sweeping)
    }

    /// Same, but the hook exits non-zero, so the strict declaration must
    /// abort the update and roll the installation back.
    fn with_failing_post_install_hook() -> Self {
        Self::build(HookFixture::Failing)
    }

    /// Incoming contract whose hook path can never resolve.
    fn with_unresolvable_post_install_hook() -> Self {
        Self::build(HookFixture::InvalidPath)
    }

    fn build(hook: HookFixture) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let system_prefix = root.join("system");
        let home = root.join("home");
        let data_home = root.join("xdg-data");
        let config_home = root.join("xdg-config");
        let state_home = root.join("xdg-state");
        let cache_home = root.join("xdg-cache");
        let runtime_dir = root.join("xdg-runtime");
        let user_layout = FsLayout::user_with_overrides(
            home.clone(),
            Some(data_home.clone()),
            Some(config_home.clone()),
            Some(state_home.clone()),
            Some(cache_home.clone()),
            Some(runtime_dir.clone()),
        );
        let system_layout = FsLayout::system(Some(system_prefix.clone()));

        // The v0.1.0 adapter bundle, as the original install laid it.
        let bundle_root = user_layout
            .datadir
            .join("adapters")
            .join(COMPONENT)
            .join("openclaw");
        std::fs::create_dir_all(&bundle_root).expect("bundle root");
        std::fs::write(bundle_root.join("plugin.json"), b"v1").expect("v1 bundle");
        let enable_digest = receipt_digest(&bundle_root).expect("enable-time digest");

        // The installed binary and its manifest snapshot.
        std::fs::create_dir_all(&user_layout.bin_dir).expect("bin dir");
        let bin = user_layout.bin_dir.join(COMPONENT);
        std::fs::write(&bin, b"v1 binary\n").expect("write bin");
        let snapshot =
            FsLayout::component_manifest_snapshot_path(&user_layout.state_dir, COMPONENT);
        std::fs::create_dir_all(snapshot.parent().expect("snapshot parent")).expect("snapshot dir");
        std::fs::write(&snapshot, manifest("0.1.0", false)).expect("write snapshot");

        // State: the component owns the binary, the snapshot, and the
        // bundle file; the receipt carries the enable-time bundle digest.
        let sha = |path: &Path| {
            use sha2::{Digest, Sha256};
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(path).expect("read owned file"))
            )
        };
        let object = InstalledObject {
            kind: ObjectKind::Component,
            name: COMPONENT.to_string(),
            version: "0.1.0".to_string(),
            status: ObjectStatus::Installed,
            manifest_digest: None,
            distribution_source: Some("https://repo.example/raw/stale-demo-0.1.0.tar.gz".into()),
            raw_package: None,
            install_backend: Some("raw".to_string()),
            ownership: Some(Ownership::RawManaged),
            rpm_metadata: None,
            installed_at: "2026-07-01T00:00:00Z".to_string(),
            last_operation_id: None,
            managed: true,
            adopted: false,
            subscription_scope: SubscriptionScope::None,
            enabled_features: Vec::new(),
            component_refs: Vec::new(),
            files: vec![
                OwnedFile {
                    path: bin.clone(),
                    owner: FileOwner::Anolisa,
                    sha256: Some(sha(&bin)),
                    kind: OwnedFileKind::File,
                    referent: None,
                    mode: None,
                    capabilities: Vec::new(),
                },
                OwnedFile {
                    path: snapshot.clone(),
                    owner: FileOwner::Anolisa,
                    sha256: None,
                    kind: OwnedFileKind::File,
                    referent: None,
                    mode: None,
                    capabilities: Vec::new(),
                },
                OwnedFile {
                    path: bundle_root.join("plugin.json"),
                    owner: FileOwner::Anolisa,
                    sha256: Some(sha(&bundle_root.join("plugin.json"))),
                    kind: OwnedFileKind::File,
                    referent: None,
                    mode: None,
                    capabilities: Vec::new(),
                },
            ],
            external_modified_files: Vec::new(),
            services: Vec::new(),
            health: Vec::new(),
            provisioned_packages: Vec::new(),
        };
        let claim = AdapterClaim {
            claim_schema: CLAIM_SCHEMA_VERSION,
            component: COMPONENT.to_string(),
            framework: "openclaw".to_string(),
            plugin_id: None,
            adapter_type: Some("plugin".to_string()),
            enabled_at: "2026-07-01T00:00:00Z".to_string(),
            resource_root: bundle_root.clone(),
            bundle_digest: Some(enable_digest.clone()),
            driver_schema: DRIVER_SCHEMA_VERSION,
            status: ClaimStatus::Enabled,
            notices: Vec::new(),
            resources: Vec::new(),
            driver_payload: DriverPayload::Cosh(CoshClaim {
                extension_dir_resource: "cosh_extension_dir".to_string(),
            }),
        };
        std::fs::create_dir_all(&user_layout.state_dir).expect("state dir");
        InstalledState {
            install_mode: StateInstallMode::User,
            prefix: user_layout.prefix.clone(),
            objects: vec![object],
            adapter_claims: vec![claim],
            ..InstalledState::default()
        }
        .save(&user_layout.state_dir.join("installed.toml"))
        .expect("user state");
        write_state(&system_layout, StateInstallMode::System, Vec::new());

        // A raw repo publishing v0.2.0 with a changed bundle.
        let hook_path = format!("share/anolisa/hooks/{COMPONENT}/post-install.sh");
        let mut entries: Vec<(&str, &[u8])> = Vec::new();
        let incoming_manifest = manifest_with_hook("0.2.0", true, hook);
        entries.push((".anolisa/component.toml", incoming_manifest.as_bytes()));
        let bin_entry = format!("bin/{COMPONENT}");
        entries.push((bin_entry.as_str(), b"v2 binary\n"));
        let bundle_entry = format!("adapters/{COMPONENT}/openclaw/plugin.json");
        entries.push((bundle_entry.as_str(), b"v2"));
        match hook {
            HookFixture::Sweeping => entries.push((hook_path.as_str(), POST_INSTALL_HOOK)),
            HookFixture::Failing => entries.push((hook_path.as_str(), FAILING_HOOK)),
            // InvalidPath ships no script: the path never resolves.
            HookFixture::None | HookFixture::InvalidPath => {}
        }
        let artifact = tar_gz(&entries);
        publish_raw_repo(&root.join("repo"), &user_layout, "0.2.0", &artifact);

        Self {
            _tmp: tmp,
            system_prefix,
            home,
            data_home,
            config_home,
            state_home,
            cache_home,
            runtime_dir,
            user_layout,
            bundle_root,
            enable_digest,
        }
    }

    fn run_update(&self, flags: &[&str]) -> Output {
        let prefix = self.system_prefix.to_string_lossy();
        let mut args: Vec<&str> = Vec::new();
        args.extend_from_slice(flags);
        args.extend([
            "--install-mode",
            "user",
            "--prefix",
            &prefix,
            "update",
            COMPONENT,
        ]);
        common::run_with_path_env(
            &args,
            &[
                ("HOME", self.home.as_path()),
                ("XDG_DATA_HOME", self.data_home.as_path()),
                ("XDG_CONFIG_HOME", self.config_home.as_path()),
                ("XDG_STATE_HOME", self.state_home.as_path()),
                ("XDG_CACHE_HOME", self.cache_home.as_path()),
                ("XDG_RUNTIME_DIR", self.runtime_dir.as_path()),
            ],
        )
    }

    fn load_store(&self) -> StateStore {
        StateStore::load(
            &self.user_layout.state_dir.join("installed.toml"),
            privilege::effective_uid(),
        )
        .expect("load state")
    }

    /// The persisted receipt's enable-time digest.
    fn persisted_bundle_digest(&self) -> Option<String> {
        self.load_store()
            .find_adapter_claim(COMPONENT, "openclaw")
            .expect("receipt must survive")
            .bundle_digest
            .clone()
    }
}

fn write_state(layout: &FsLayout, mode: StateInstallMode, objects: Vec<InstalledObject>) {
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    InstalledState {
        install_mode: mode,
        prefix: layout.prefix.clone(),
        objects,
        ..InstalledState::default()
    }
    .save(&layout.state_dir.join("installed.toml"))
    .expect("state");
}

fn publish_raw_repo(root: &Path, layout: &FsLayout, version: &str, artifact: &[u8]) {
    use sha2::{Digest, Sha256};
    // `base_url` must point at the `v1/` distribution root that holds
    // `index.toml` (repo_config::raw_root).
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dir");
    let artifact_name = format!("{COMPONENT}.tar.gz");
    std::fs::write(v1.join(&artifact_name), artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(artifact));
    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{COMPONENT}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = ["system", "user"]
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        format!(
            "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"file://{}\"\n",
            v1.display()
        ),
    )
    .expect("write repo.toml");
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn update_human_lists_stale_adapter_with_recovery_command() {
    let fixture = UpdateFixture::new();
    let output = fixture.run_update(&[]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("stale-demo/openclaw"), "{out}");
    assert!(
        out.contains("resource bundle changed since enable"),
        "{out}"
    );
    assert!(
        out.contains("anolisa adapter enable stale-demo openclaw"),
        "the notice must carry the exact recovery command: {out}"
    );
    // The update itself replaced the bundle ...
    assert_eq!(
        std::fs::read(fixture.bundle_root.join("plugin.json")).expect("read bundle"),
        b"v2"
    );
    // ... but the receipt keeps its enable-time digest: only an explicit
    // `adapter enable` may clear the drift.
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str()),
        "update must report the drift, not erase it"
    );
}

#[test]
fn update_quiet_suppresses_adapter_notices() {
    let fixture = UpdateFixture::new();
    let output = fixture.run_update(&["--quiet"]);
    assert_success(&output);
    let out = stdout(&output);
    assert!(
        !out.contains("resource bundle changed") && !out.contains("adapter enable"),
        "--quiet must not print adapter notices: {out}"
    );
}

#[test]
fn update_json_carries_adapter_actions() {
    let fixture = UpdateFixture::new();
    let output = fixture.run_update(&["--json"]);
    assert_success(&output);
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"]["updated"], true);
    assert_eq!(
        envelope["data"]["adapter_actions"],
        serde_json::json!([{
            "component": "stale-demo",
            "framework": "openclaw",
            "reason": "resource bundle changed since enable",
            "command": "anolisa adapter enable stale-demo openclaw",
        }]),
        "{envelope}"
    );
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str()),
        "the receipt must survive the update unchanged"
    );
}

#[test]
fn update_dry_run_reports_no_adapter_actions_and_changes_nothing() {
    let fixture = UpdateFixture::new();
    let output = fixture.run_update(&["--json", "--dry-run"]);
    assert_success(&output);
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["updated"], false);
    assert_eq!(
        envelope["data"]["adapter_actions"],
        serde_json::json!([]),
        "dry-run must not claim a post-update mismatch has occurred: {envelope}"
    );
    assert_eq!(
        std::fs::read(fixture.bundle_root.join("plugin.json")).expect("read bundle"),
        b"v1",
        "dry-run must not touch the bundle"
    );
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str())
    );
}

/// Regression for alibaba/anolisa#2252: a raw upgrade must be able to clear
/// artifacts the payload does not own.
///
/// `remove_owned_files` only drops files the prior record tracked, so Python
/// bytecode a Hook wrote beside its installed source survives the file
/// replacement and keeps the adapter's bundle digest away from its
/// enable-time value. The component's `post_install` hook is what removes it,
/// which requires the update plan to carry the hook slot at all.
#[test]
fn update_runs_post_install_hook_and_clears_untracked_bytecode() {
    let fixture = UpdateFixture::with_post_install_hook();

    // Bytecode as a Hook run would have left it: inside the bundle, absent
    // from the record's file list, so nothing else in the update removes it.
    let cache = fixture.bundle_root.join("hooks").join("__pycache__");
    std::fs::create_dir_all(&cache).expect("cache dir");
    std::fs::write(
        cache.join("hook_config.cpython-311.pyc"),
        b"\xcb\r\r\nstale",
    )
    .expect("stale bytecode");

    let output = fixture.run_update(&[]);
    assert_success(&output);

    let marker = fixture
        .user_layout
        .datadir
        .join("hooks")
        .join(COMPONENT)
        .join("ran.marker");
    assert!(
        marker.exists(),
        "post_install hook must run on the update path; stdout: {}",
        stdout(&output)
    );
    assert!(
        !cache.exists(),
        "stale bytecode must be gone after the upgrade"
    );
    // The upgrade replaced the bundle, so the receipt legitimately reports
    // drift — what must not happen is drift caused by leftover bytecode.
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str()),
        "the receipt itself must survive the update unchanged"
    );
}

/// A strict `post_install` hook that fails must abort the update and restore
/// the prior installation — files, manifest snapshot, and record version.
///
/// `RawInstallOps` has its own strict-hook coverage, which says nothing about
/// the replay port: only this path exercises `remove_owned_files` +
/// `place_files` rollback with the backups the replay took.
#[test]
fn update_rolls_back_when_strict_post_install_hook_fails() {
    let fixture = UpdateFixture::with_failing_post_install_hook();

    let output = fixture.run_update(&[]);
    assert!(
        !output.status.success(),
        "a failed strict hook must fail the update; stdout: {}",
        stdout(&output)
    );

    // Prior payload restored, not the v0.2.0 one the hook aborted.
    assert_eq!(
        std::fs::read(fixture.user_layout.bin_dir.join(COMPONENT)).expect("read bin"),
        b"v1 binary\n",
        "the prior binary must be restored"
    );
    assert_eq!(
        std::fs::read(fixture.bundle_root.join("plugin.json")).expect("read bundle"),
        b"v1",
        "the prior adapter bundle must be restored"
    );

    // The record still describes the installed version, and the receipt is
    // untouched, so `adapter status` keeps reporting against v0.1.0.
    let store = fixture.load_store();
    let record = store
        .find(anolisa_core::ObjectKind::Component, COMPONENT)
        .expect("record must survive");
    let version = match &record.binding {
        anolisa_core::domain::ProviderBinding::Owned { artifact } => artifact.version.clone(),
        other => panic!("expected an owned binding, got {other:?}"),
    };
    assert_eq!(version, "0.1.0", "a rolled-back update must not bump");
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str())
    );

    // The hook that failed must not have been left behind as an installed
    // file of a version that never committed.
    let snapshot =
        FsLayout::component_manifest_snapshot_path(&fixture.user_layout.state_dir, COMPONENT);
    let restored = std::fs::read_to_string(&snapshot).expect("read snapshot");
    assert!(
        !restored.contains("post_install"),
        "the prior manifest snapshot must be restored: {restored}"
    );
}

/// A contract-only error in the incoming hook declaration must abort before
/// the replay modifies anything.
///
/// The hook specs are resolved during `download_verify`, which runs before
/// `remove_owned_files` and `place_files`. Resolving them later would mean an
/// unresolvable placeholder tore down the old payload first and only then
/// entered rollback — the fresh-install path's "contract error aborts before
/// IO" property, lost on update.
#[test]
fn update_rejects_unresolvable_hook_path_before_touching_the_host() {
    let fixture = UpdateFixture::with_unresolvable_post_install_hook();
    let bin = fixture.user_layout.bin_dir.join(COMPONENT);
    let placed_before = std::fs::read(&bin).expect("read bin");

    let output = fixture.run_update(&["--json"]);
    assert!(
        !output.status.success(),
        "an unresolvable hook path must fail the update; stdout: {}",
        stdout(&output)
    );

    // The step it failed at is the point of this test: `download and verify
    // artifact` means the contract was rejected before any host mutation.
    // Failing at the hook step instead would mean the old payload had already
    // been removed and the new one placed.
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let reason = envelope["error"]["reason"].as_str().expect("reason");
    assert!(
        reason.contains("failed at 'download and verify artifact'"),
        "contract errors must abort at download-verify, not mid-replacement: {reason}"
    );
    assert!(
        reason.contains("unknown placeholder 'no_such_placeholder'"),
        "{reason}"
    );

    assert_eq!(
        std::fs::read(&bin).expect("read bin"),
        placed_before,
        "the prior binary must never have been replaced"
    );
    assert_eq!(
        std::fs::read(fixture.bundle_root.join("plugin.json")).expect("read bundle"),
        b"v1",
        "the prior bundle must never have been replaced"
    );
    assert!(
        !fixture
            .user_layout
            .datadir
            .join("hooks")
            .join(COMPONENT)
            .exists(),
        "no v0.2.0 payload may have landed"
    );
    assert_eq!(
        fixture.persisted_bundle_digest().as_deref(),
        Some(fixture.enable_digest.as_str())
    );
}
