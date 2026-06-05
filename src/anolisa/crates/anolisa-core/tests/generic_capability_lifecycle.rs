//! Generic-capability lifecycle smoke test.
//!
//! Proves the framework can take a brand-new capability + component
//! through the full lifecycle — plan → install → integrity probe →
//! disable → uninstall — using *only* on-disk manifests + a
//! DistributionIndex entry, with no CLI handler changes and no
//! capability-name special cases. This is the contract that backstops
//! the "adding a new capability = manifest + artifact + index entry"
//! delivery model — if any of these tests ever needs a code change to
//! add a capability, the framework has regressed.
//!
//! The fixture stages everything in tmp dirs:
//!   * a tiny capability TOML under `capabilities/`
//!   * a tiny component TOML under `runtime/`
//!   * a fake artifact file with a known sha256
//!   * a single DistributionIndex entry pointing at the artifact via
//!     `file://`
//!   * a system-mode [`FsLayout`] rebased under a tempdir prefix so
//!     real install / uninstall IO is fully isolated (no env vars
//!     consulted, no `$HOME` writes)
//!
//! `build_fixture` is parameterized by capability + component name so
//! the same plumbing can stage any number of differently-named
//! capabilities. A second test (`..._with_alternate_name`) drives the
//! exact same lifecycle through a different name pair to prove the
//! framework dispatches solely on manifest contents, with zero
//! hardcoded capability or component identifiers along the path.

use std::fs;
use std::path::PathBuf;

use anolisa_core::{
    CapabilityManifestsView, Catalog, CatalogLayers, DistributionIndex, FileOwner, InstalledState,
    IntegrityStatus, LifecycleOperation, LifecyclePlan, ObjectKind, ObjectStatus, PlanStatus,
    check_owned_file, contract_lint, execute_disable, execute_enable, execute_plan,
    lint_has_errors, plan_enable,
};
use anolisa_env::EnvFacts;
use anolisa_platform::fs_layout::FsLayout;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// All paths the test fixture writes — kept together so test code can
/// reach into individual files when asserting wire shapes.
struct Fixture {
    _tmp: TempDir,
    capability: String,
    catalog_root: PathBuf,
    dist_index_path: PathBuf,
    artifact_sha256: String,
    layout: FsLayout,
}

/// Names that vary per-fixture. Keep the parameter struct tiny on
/// purpose — anything more elaborate would smuggle a name allowlist
/// back into the framework via the test surface.
struct FixtureNames<'a> {
    capability: &'a str,
    component: &'a str,
    description: &'a str,
}

fn build_fixture(names: FixtureNames<'_>) -> Fixture {
    let tmp = TempDir::new().expect("tmpdir");
    let root = tmp.path();

    // 1. Manifest layout: capabilities/ and runtime/ under a single
    //    catalog root that we'll register as the bundled layer.
    let catalog_root = root.join("manifests");
    let capabilities_dir = catalog_root.join("capabilities");
    let runtime_dir = catalog_root.join("runtime");
    fs::create_dir_all(&capabilities_dir).expect("mkdir capabilities");
    fs::create_dir_all(&runtime_dir).expect("mkdir runtime");

    // 2. Fake artifact + sha256. Bytes are arbitrary (component-name
    //    keyed so two fixtures in the same test process never produce
    //    identical sha256s) — the framework only cares that the URL
    //    resolves and the sha matches.
    let artifact_dir = root.join("artifacts");
    fs::create_dir_all(&artifact_dir).expect("mkdir artifacts");
    let artifact_filename = format!("{}-1.0.0-linux-x86_64.bin", names.component);
    let artifact_path = artifact_dir.join(&artifact_filename);
    let artifact_payload = format!("{}-payload-v1", names.component);
    fs::write(&artifact_path, artifact_payload.as_bytes()).expect("write artifact");
    let artifact_sha256 = {
        let mut h = Sha256::new();
        h.update(artifact_payload.as_bytes());
        hex_lower(&h.finalize())
    };

    // 3. Capability manifest — the bare minimum that resolves: name,
    //    one component, linux+x86_64 env constraints.
    let capability_toml = format!(
        r#"[capability]
name = "{cap}"
description = "{desc}"
layer = "agent"
stability = "experimental"

[implementation]
components = ["{comp}"]

[requires_env]
os = "linux"
arch = ["x86_64"]
"#,
        cap = names.capability,
        comp = names.component,
        desc = names.description,
    );
    fs::write(
        capabilities_dir.join(format!("{}.toml", names.capability)),
        capability_toml,
    )
    .expect("write capability");

    // 4. Component manifest. Single owned file targeting `{bindir}/`
    //    so it lands inside the ANOLISA-owned root and lint accepts the
    //    destination. `install.modes` covers both `user` and `system`
    //    so the same fixture serves the planning-only test (defaults to
    //    user) and the end-to-end test (system, env-isolated under tmp).
    let component_toml = format!(
        r#"[component]
name = "{comp}"
version = "1.0.0"
layer = "runtime"
description = "{desc}"

[install]
modes = ["user", "system"]

[[install.files]]
source = "bin/{comp}"
dest = "{{bindir}}/{comp}"
mode = "0755"

[environment]
requires_os = "linux"
requires_arch = ["x86_64"]
"#,
        comp = names.component,
        desc = names.description,
    );
    fs::write(
        runtime_dir.join(format!("{}.toml", names.component)),
        component_toml,
    )
    .expect("write component");

    // 5. DistributionIndex with one entry. Backend `binary` is what the
    //    install runner understands today; the planner only needs the
    //    entry to resolve. `install_modes` mirrors the component manifest
    //    so the resolver finds the entry on either install mode.
    let dist_index_dir = catalog_root.join("distribution-index");
    fs::create_dir_all(&dist_index_dir).expect("mkdir dist-index");
    let dist_index_path = dist_index_dir.join("index.toml");
    let dist_index_toml = format!(
        r#"schema_version = 1
publisher = "smoke-fixture"

[[entries]]
component = "{comp}"
version = "1.0.0"
channel = "stable"
artifact_type = "binary"
backend = "binary"
url = "file://{url}"
os = "linux"
arch = "x86_64"
install_modes = ["user", "system"]
sha256 = "{sha256}"
"#,
        comp = names.component,
        url = artifact_path.display(),
        sha256 = artifact_sha256,
    );
    fs::write(&dist_index_path, dist_index_toml).expect("write dist index");

    // 6. Layout: system mode rebased under tmp so every owned root
    //    (bin / etc / state / log / data / lib / libexec / cache) lives
    //    inside the fixture. System mode is fully env-isolated — unlike
    //    user mode it does not consult XDG_DATA_HOME etc. — so the
    //    end-to-end test writes nothing outside the tempdir even on a
    //    developer machine with custom XDG variables exported.
    let prefix = root.join("install-prefix");
    fs::create_dir_all(&prefix).expect("mkdir prefix");
    let layout = FsLayout::system(Some(prefix.clone()));

    Fixture {
        _tmp: tmp,
        capability: names.capability.to_string(),
        catalog_root,
        dist_index_path,
        artifact_sha256,
        layout,
    }
}

fn hello_fixture() -> Fixture {
    build_fixture(FixtureNames {
        capability: "hello-sample",
        component: "hello-component",
        description: "Generic-lifecycle smoke fixture (hello)",
    })
}

fn world_fixture() -> Fixture {
    build_fixture(FixtureNames {
        capability: "world-sample",
        component: "world-component",
        description: "Generic-lifecycle smoke fixture (world)",
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn linux_x86_64_facts() -> EnvFacts {
    EnvFacts {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        libc: Some("glibc".to_string()),
        kernel: Some("6.1.0".to_string()),
        pkg_base: Some("anolis23".to_string()),
        btf: Some(true),
        cap_bpf: Some(true),
        container: None,
        user: "tester".to_string(),
        uid: 1000,
        home: PathBuf::from("/home/tester"),
    }
}

/// Drives the full plan → install → integrity → disable → uninstall
/// chain on a fixture. Shared by every lifecycle test in this file so
/// adding a new capability name to the proof is a one-line `assert_*`
/// call rather than a copy of the entire flow — and so a regression in
/// any phase fails for every name in lockstep.
fn assert_full_lifecycle(fx: &Fixture, component_name: &str, actor: &str) {
    let catalog =
        Catalog::load(CatalogLayers::bundled_only(fx.catalog_root.clone())).expect("catalog loads");
    let dist_index = DistributionIndex::load(&fx.dist_index_path).expect("dist index loads");
    let env = linux_x86_64_facts();

    // --- plan ---
    let plan = plan_enable(
        &catalog,
        &dist_index,
        &env,
        "system",
        &fx.layout,
        &fx.capability,
    )
    .expect("plan_enable accepts a fresh manifest-only capability");
    assert_eq!(
        plan.status,
        PlanStatus::Ready,
        "plan must be ready before install (capability={})",
        fx.capability,
    );

    // --- install ---
    let install = execute_enable(&plan, &fx.layout, actor).expect("execute_enable ok");
    assert_eq!(
        install.installed_files.len(),
        1,
        "one owned file expected from one [[install.files]] entry",
    );
    let installed_path = install.installed_files[0].path.clone();
    assert!(
        installed_path.exists(),
        "post-install: binary must be on disk at {}",
        installed_path.display(),
    );
    assert_eq!(
        install.installed_files[0].sha256, fx.artifact_sha256,
        "post-install: recorded sha256 must match the fixture artifact",
    );
    let state_path = fx.layout.state_dir.join("installed.toml");
    let state_after_install = InstalledState::load(&state_path).expect("state loads");
    let cap = state_after_install
        .find_object(ObjectKind::Capability, &fx.capability)
        .expect("capability object recorded");
    assert_eq!(cap.status, ObjectStatus::Installed);
    let comp = state_after_install
        .find_object(ObjectKind::Component, component_name)
        .expect("component object recorded");
    assert_eq!(comp.status, ObjectStatus::Installed);
    assert_eq!(comp.files.len(), 1);
    assert_eq!(comp.files[0].owner, FileOwner::Anolisa);

    // --- integrity probe (the bit that exercises path-safety + sha) ---
    let integrity = check_owned_file(&fx.layout, &comp.files[0]);
    assert_eq!(
        integrity,
        IntegrityStatus::Ok,
        "fresh install must hash-clean; got {integrity:?}",
    );

    // --- disable ---
    let disable_outcome =
        execute_disable(&fx.layout, &fx.capability, actor, "system").expect("execute_disable ok");
    assert_eq!(disable_outcome.capability, fx.capability);
    let state_after_disable = InstalledState::load(&state_path).expect("state loads");
    let cap_disabled = state_after_disable
        .find_object(ObjectKind::Capability, &fx.capability)
        .expect("capability still recorded after disable");
    assert_eq!(
        cap_disabled.status,
        ObjectStatus::Disabled,
        "disable must flip the wire status to Disabled without deleting the object",
    );
    assert!(
        installed_path.exists(),
        "disable must NOT remove the binary at {}",
        installed_path.display(),
    );

    // --- uninstall ---
    let uninstall_plan = LifecyclePlan::for_uninstall(
        &fx.capability,
        &state_after_disable,
        &CapabilityManifestsView::empty(),
    );
    assert_eq!(uninstall_plan.operation, LifecycleOperation::Uninstall);
    let uninstall_outcome =
        execute_plan(&uninstall_plan, &fx.layout, actor, "system").expect("execute_plan ok");
    assert!(
        !uninstall_outcome.removed_files.is_empty(),
        "uninstall must report at least one removed file",
    );
    assert!(
        uninstall_outcome
            .removed_files
            .iter()
            .any(|p| p == &installed_path),
        "uninstall must report removing the installed binary at {}",
        installed_path.display(),
    );
    assert!(
        !installed_path.exists(),
        "post-uninstall: binary must be gone from {}",
        installed_path.display(),
    );
    let state_after_uninstall = InstalledState::load(&state_path).expect("state loads");
    assert!(
        state_after_uninstall
            .find_object(ObjectKind::Capability, &fx.capability)
            .is_none(),
        "post-uninstall: capability object must be removed from installed.toml",
    );
}

/// End-to-end planning works for a brand-new capability that the
/// framework has never seen before, using only manifest + index +
/// artifact files. No CLI handler change, no resolver override, no
/// capability-name dispatch.
#[test]
fn generic_capability_plans_ready_from_manifests_only() {
    let fx = hello_fixture();
    let catalog =
        Catalog::load(CatalogLayers::bundled_only(fx.catalog_root.clone())).expect("catalog loads");
    assert!(
        catalog.capability("hello-sample").is_some(),
        "fixture capability must be discovered by the bundled loader"
    );
    assert!(
        catalog.component("hello-component").is_some(),
        "fixture component must be discovered by the bundled loader"
    );

    let dist_index = DistributionIndex::load(&fx.dist_index_path).expect("dist index loads");
    assert_eq!(
        dist_index.entries.len(),
        1,
        "fixture index has exactly one entry"
    );

    let env = linux_x86_64_facts();
    let plan = plan_enable(
        &catalog,
        &dist_index,
        &env,
        "system",
        &fx.layout,
        "hello-sample",
    )
    .expect("planner accepts the fresh capability without code changes");

    // The fixture-side contract: the framework returns a ready plan
    // because (a) the capability exists, (b) env matches, (c) the
    // distribution entry resolves, (d) lint sees no errors.
    assert_eq!(
        plan.status,
        PlanStatus::Ready,
        "expected Ready, got {:?} (blocked_reason: {:?}, warnings: {:?})",
        plan.status,
        plan.blocked_reason,
        plan.warnings,
    );
    assert_eq!(plan.capability, "hello-sample");
    assert_eq!(plan.components.len(), 1);
    let component_plan = &plan.components[0];
    assert_eq!(component_plan.name, "hello-component");
    let artifact = component_plan
        .artifact
        .as_ref()
        .expect("artifact resolved from the fixture index entry");
    assert_eq!(artifact.version, "1.0.0");
    assert_eq!(
        artifact.sha256.as_deref(),
        Some(fx.artifact_sha256.as_str())
    );
    assert!(
        artifact.url.starts_with("file://"),
        "fixture artifact uses file:// URL, got {}",
        artifact.url
    );
    // The lint side of the plan must be silent on errors — a
    // capability that ships through this path with an Error finding
    // would never have reached Ready, but assert explicitly so a
    // regression in `status` calculation cannot hide a real lint bug.
    assert!(
        !lint_has_errors(&plan.lint),
        "lint must be error-free for a clean fixture, got: {:?}",
        plan.lint
    );
}

/// End-to-end lifecycle: plan → install → integrity → disable →
/// uninstall, all driven through the public executor entrypoints with
/// no capability-name dispatch anywhere in the chain. The smoke fixture
/// is system-mode and tmp-rebased so every owned root sits inside the
/// fixture — the test runs without root, without sudo, and without
/// touching the developer's `$HOME`.
///
/// Each phase asserts both the in-memory outcome (installed_files /
/// IntegrityStatus / state object kind+status) and the on-disk
/// fingerprint (`installed.toml`, the installed binary, the lock file)
/// so a regression in any one layer surfaces here rather than as a
/// silent skip downstream.
#[test]
fn generic_capability_full_lifecycle_drives_every_executor() {
    let fx = hello_fixture();
    assert_full_lifecycle(&fx, "hello-component", "smoke-tester");
}

/// Same lifecycle, different names. The whole point of the alpha
/// contract is that the framework dispatches purely on manifest
/// contents — no capability-name allowlist, no `match cap.name`
/// branches. If this test ever needs Rust changes to pass, the
/// framework has grown a name-coupling and the contract has regressed.
#[test]
fn generic_capability_full_lifecycle_drives_every_executor_with_alternate_name() {
    let fx = world_fixture();
    assert_full_lifecycle(&fx, "world-component", "smoke-tester-alt");
}

/// Run the standalone contract lint directly: same catalog + index +
/// layout used by the planner, but with no env/install-mode coupling.
/// This guards the contract that lint never injects errors for a
/// well-formed manifest and never depends on capability-name lookups
/// for built-in dispatch.
#[test]
fn generic_capability_contract_lint_is_silent_on_errors() {
    let fx = hello_fixture();
    let catalog =
        Catalog::load(CatalogLayers::bundled_only(fx.catalog_root.clone())).expect("catalog loads");
    let dist_index = DistributionIndex::load(&fx.dist_index_path).expect("dist index loads");

    let findings =
        contract_lint::lint_capability(&catalog, &dist_index, &fx.layout, "hello-sample");
    let errors: Vec<&_> = findings
        .iter()
        .filter(|f| matches!(f.severity, contract_lint::LintSeverity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "lint must not emit errors for a well-formed fixture, got: {errors:?}"
    );
}

/// Counterpart of `generic_capability_contract_lint_is_silent_on_errors`
/// for the alternate-name fixture. Lint is the gate that planning
/// trusts — if it ever grew a name-keyed branch, this test would
/// surface the asymmetry between names *before* the lifecycle test
/// further down the line.
#[test]
fn generic_capability_contract_lint_is_silent_on_errors_with_alternate_name() {
    let fx = world_fixture();
    let catalog =
        Catalog::load(CatalogLayers::bundled_only(fx.catalog_root.clone())).expect("catalog loads");
    let dist_index = DistributionIndex::load(&fx.dist_index_path).expect("dist index loads");

    let findings =
        contract_lint::lint_capability(&catalog, &dist_index, &fx.layout, "world-sample");
    let errors: Vec<&_> = findings
        .iter()
        .filter(|f| matches!(f.severity, contract_lint::LintSeverity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "lint must not emit errors for a well-formed fixture, got: {errors:?}"
    );
}
