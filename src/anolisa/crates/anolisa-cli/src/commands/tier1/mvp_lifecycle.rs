//! End-to-end MVP lifecycle coverage (#963).
//!
//! Drives a single image-preinstalled system RPM through the whole flow —
//! adopt → update → drift status → uninstall — on **one** state file and
//! **one** evolving fake rpmdb world. Where the per-command unit tests
//! each seed their own hand-written start state, these feed *each command's real
//! output* into the next, so they catch the cross-command regressions those
//! tests assume away: a state-contract drift between what `adopt` writes and
//! what `update`/`status`/`uninstall` read, and rpmdb-evolution effects that a
//! static per-command fake cannot model.
//!
//! Scope (#963) → coverage:
//! - ① adopt under `default_backend = raw` — [`lifecycle_adopt_update_drift_then_uninstall`] (install step)
//! - ② update delegates to RPM + refreshes state — same test (update step)
//! - ④ rpm-observed uninstall — same test (uninstall step); `forget` is covered
//!   by `super::forget::tests::forget_drops_object_and_records_operation`
//! - ⑤ drift status — same test (drift step); the `Missing` counterpart is
//!   covered by `super::status::tests::probe_rpm_drift_detects_missing`
//! - ③ user-mode raw shadows system RPM — covered by
//!   `super::update::tests::user_raw_override_does_not_touch_system_rpm`; not
//!   re-done here because `common::resolve_layout` binds user mode to the real
//!   `$HOME`, so a command-layer user-mode test cannot be isolated without
//!   polluting the home dir or mutating process-global `XDG_*` (which would race
//!   other tests).

use std::cell::{Cell, RefCell};

use anolisa_core::state::{InstalledState, ObjectKind, Ownership};
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError, PackageVersion};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};

use crate::commands::common;
use crate::context::{CliContext, InstallMode};

use super::install::{InstallArgs, InstallOutcome, RpmExec, handle_one_with_exec};
use super::status::{RpmDrift, probe_rpm_drift};
use super::uninstall::{UninstallArgs, handle_with_deps};
use super::update::update_component_with_deps;

/// An in-memory rpmdb + dnf that **mutates as commands act on it**, so one fake
/// backs the whole flow: it answers [`PackageQuery`] (adopt probe, status drift)
/// and runs [`PackageTransaction`] (update/remove).
///
/// `installed` is the live rpmdb view. A successful [`update`](PackageTransaction::update)
/// advances it to `upgrade_to` (dnf moving the version forward); the
/// `simulate_*` helpers model out-of-band `dnf`/`rpm` changes that ANOLISA state
/// does not know about, which is exactly what the drift/missing probes must
/// catch.
struct RpmWorld {
    package: String,
    installed: RefCell<Option<PackageInfo>>,
    origin: String,
    /// rpmdb view after a successful in-band `update` (models dnf advancing).
    upgrade_to: Option<PackageInfo>,
    update_calls: Cell<usize>,
    remove_calls: Cell<usize>,
}

impl RpmWorld {
    /// A world where `package` is preinstalled at `info`, attributed to `origin`.
    fn preinstalled(package: &str, info: PackageInfo, origin: &str) -> Self {
        Self {
            package: package.to_string(),
            installed: RefCell::new(Some(info)),
            origin: origin.to_string(),
            upgrade_to: None,
            update_calls: Cell::new(0),
            remove_calls: Cell::new(0),
        }
    }

    /// rpmdb view a subsequent in-band `update` advances to.
    fn upgrading_to(mut self, info: PackageInfo) -> Self {
        self.upgrade_to = Some(info);
        self
    }

    /// Model an out-of-band `dnf update`/`downgrade`: rpmdb moves, ANOLISA state
    /// is untouched, so a later status probe must report drift.
    fn simulate_dnf_upgrade(&self, info: PackageInfo) {
        *self.installed.borrow_mut() = Some(info);
    }

    /// Current rpmdb view for `package` (`None` for any other name).
    fn current(&self, package: &str) -> Option<PackageInfo> {
        (package == self.package)
            .then(|| self.installed.borrow().clone())
            .flatten()
    }
}

impl PackageQuery for RpmWorld {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        Ok(self.current(package))
    }
    fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        Ok(Vec::new())
    }
    fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
        Ok((package == self.package).then(|| self.origin.clone()))
    }
    fn what_provides_installed(&self, _capability: &str) -> Result<Vec<String>, PackageQueryError> {
        Ok(Vec::new())
    }
}

impl PackageTransaction for RpmWorld {
    fn install(&self, _package: &str) -> Result<(), PackageTransactionError> {
        // The MVP flow adopts a preinstalled RPM; reaching dnf install is a bug.
        panic!("MVP lifecycle adopts a preinstalled RPM; dnf install must not run");
    }
    fn update(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.update_calls.set(self.update_calls.get() + 1);
        assert_eq!(package, self.package, "update targeted the wrong package");
        if let Some(next) = &self.upgrade_to {
            *self.installed.borrow_mut() = Some(next.clone());
        }
        Ok(())
    }
    fn remove(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.remove_calls.set(self.remove_calls.get() + 1);
        assert_eq!(package, self.package, "remove targeted the wrong package");
        *self.installed.borrow_mut() = None;
        Ok(())
    }
}

fn pkg_info(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
    PackageInfo {
        name: name.to_string(),
        version: PackageVersion {
            epoch: None,
            version: version.to_string(),
            release: release.map(str::to_string),
        },
        arch: arch.to_string(),
        origin: None,
    }
}

/// A system-mode ctx whose tempdir holds a `repo.toml` with
/// `default_backend = raw` — so the install entry point exercises scope ①: raw
/// is the default backend, yet a preinstalled RPM is still adopted.
fn system_ctx_with_raw_repo() -> (tempfile::TempDir, CliContext) {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let prefix = tmp.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"https://example.com/anolisa\"\n",
    )
    .expect("write repo.toml");
    let ctx = CliContext {
        install_mode: InstallMode::System,
        prefix: Some(prefix),
        json: false,
        dry_run: false,
        verbose: false,
        quiet: true,
        no_color: true,
    };
    (tmp, ctx)
}

fn load_state(ctx: &CliContext) -> InstalledState {
    let layout = common::resolve_layout(ctx);
    InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
}

fn install_args(component: &str, package: Option<&str>) -> InstallArgs {
    InstallArgs {
        component: Some(component.to_string()),
        all: false,
        fail_fast: false,
        version: None,
        backend: None,
        repo: None,
        package: package.map(str::to_string),
    }
}

fn uninstall_args(component: &str) -> UninstallArgs {
    UninstallArgs {
        component: component.to_string(),
        purge: false,
        remove_system_package: false,
        force: false,
    }
}

/// Adopt a preinstalled RPM whose package name (`copilot-shell`) deliberately
/// differs from the component name (`cosh`) — they share no `anolisa-` prefix,
/// so the package identity can only flow through the `rpm_metadata.package_name`
/// adopt writes. The evolving [`RpmWorld`] answers *only* for `copilot-shell`,
/// so update/status/uninstall pass solely by consuming that recorded name
/// rather than re-deriving `anolisa-{component}`. Drives adopt → update (rpmdb
/// advances, state refreshes) → drift after an out-of-band dnf upgrade →
/// uninstall (state dropped, system RPM left); each step consumes the previous
/// step's *real* persisted state and the same world.
#[test]
fn lifecycle_adopt_update_drift_then_uninstall() {
    let (_tmp, ctx) = system_ctx_with_raw_repo();
    let pkg = "copilot-shell";
    let component = "cosh";
    let world = RpmWorld::preinstalled(
        pkg,
        pkg_info(pkg, "2.3.0", Some("1.al8"), "x86_64"),
        "@System",
    )
    .upgrading_to(pkg_info(pkg, "2.4.0", Some("1.al8"), "x86_64"));

    // ① The install entry point under default_backend=raw, pinned with
    //    `--package`, adopts the preinstalled RPM rather than fetching raw.
    let exec = RpmExec::new(&world, &world, true);
    let outcome = handle_one_with_exec(
        component.to_string(),
        install_args(component, Some(pkg)),
        &ctx,
        &exec,
    )
    .expect("adopt via install entry point");
    assert_eq!(outcome, InstallOutcome::Adopted);
    let obj = load_state(&ctx)
        .find_object(ObjectKind::Component, component)
        .cloned()
        .expect("component recorded after adopt");
    assert_eq!(obj.ownership, Some(Ownership::RpmObserved));
    assert_eq!(obj.install_backend.as_deref(), Some("rpm"));
    assert!(!obj.managed, "rpm-observed is not ANOLISA-managed");
    assert!(obj.adopted);
    assert!(obj.files.is_empty(), "adopt writes no owned files");
    let meta = obj.rpm_metadata.clone().expect("rpm metadata");
    assert_eq!(
        meta.package_name, pkg,
        "adopt records the real RPM package name, not anolisa-{component}",
    );
    assert_eq!(meta.evr.as_deref(), Some("2.3.0-1.al8"));

    // ② update delegates dnf and refreshes the recorded EVR from rpmdb, keeping
    //    rpm-observed ownership. The world answers only for `copilot-shell`, so
    //    this passes only if update targets the adopted package name.
    update_component_with_deps(component, &ctx, &world, &world, true).expect("update ok");
    assert_eq!(world.update_calls.get(), 1, "dnf update ran once");
    let obj = load_state(&ctx)
        .find_object(ObjectKind::Component, component)
        .cloned()
        .expect("component present after update");
    assert_eq!(
        obj.ownership,
        Some(Ownership::RpmObserved),
        "update keeps observed ownership",
    );
    let meta = obj.rpm_metadata.clone().expect("rpm metadata");
    assert_eq!(
        meta.evr.as_deref(),
        Some("2.4.0-1.al8"),
        "state EVR refreshed from rpmdb after update",
    );

    // ⑤ an out-of-band dnf upgrade diverges rpmdb from the recorded EVR → drift.
    //    The probe resolves the package via the recorded `package_name`.
    world.simulate_dnf_upgrade(pkg_info(pkg, "2.5.0", Some("1.al8"), "x86_64"));
    match probe_rpm_drift(&meta, &world) {
        Some(RpmDrift::Drifted { .. }) => {}
        _ => panic!("expected drift after an out-of-band dnf upgrade"),
    }

    // ④ rpm-observed uninstall without --remove-system-package drops only ANOLISA
    //    state and leaves the system RPM in place.
    handle_with_deps(uninstall_args(component), &ctx, &world, &world, true).expect("uninstall ok");
    assert_eq!(
        world.remove_calls.get(),
        0,
        "rpm-observed default must not dnf remove",
    );
    assert!(world.current(pkg).is_some(), "system RPM left installed");
    assert!(
        load_state(&ctx)
            .find_object(ObjectKind::Component, component)
            .is_none(),
        "ANOLISA state dropped",
    );
}
