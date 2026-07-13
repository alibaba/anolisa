//! Shared test helpers, fixture builders, and integration test modules
//! for the `install` command.
//!
//! Unit tests are inlined in each source module under `#[cfg(test)] mod tests`.
//! Integration tests are split by theme into sub-modules here:

use super::*;

use anolisa_core::state::InstalledState;
use anolisa_platform::fs_layout::FsLayout;
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};
use anolisa_platform::pkg_transaction::{PackageTransaction, PackageTransactionError};

use crate::commands::common;
use crate::context::InstallMode;
use crate::repo_config::RepoConfig;
use crate::resolution::rpm_component_provide;
use anolisa_platform::pkg_query::PackageVersion;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use tar::{Builder, Header};
use tempfile::tempdir;

pub fn ctx_with_prefix(json: bool, prefix: Option<PathBuf>) -> CliContext {
    CliContext {
        install_mode: if prefix.is_some() {
            InstallMode::System
        } else {
            InstallMode::User
        },
        prefix,
        json,
        dry_run: false,
        verbose: false,
        quiet: true, // suppress stdout during tests
        no_color: true,
    }
}

pub fn args(component: &str) -> InstallArgs {
    InstallArgs {
        component: Some(component.to_string()),
        all: false,
        fail_fast: false,
        version: None,
        backend: None,
        repo: None,
        package: None,
    }
}

pub fn handle_with_fake_rpm(args: InstallArgs, ctx: &CliContext) -> Result<(), CliError> {
    let component = args
        .component
        .clone()
        .expect("single-component install test sets args.component");
    handle_one_with_query(component, args, ctx, &FakeQuery::default()).map(|_| ())
}

pub fn toml_string_array(values: &[&str]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
    format!("[{}]", quoted.join(", "))
}

pub fn component_manifest_toml(component: &str, version: &str, modes: &[&str]) -> String {
    component_manifest_toml_with_conflicts(component, version, modes, &[])
}

pub fn component_manifest_toml_with_conflicts(
    component: &str,
    version: &str,
    modes: &[&str],
    conflicts: &[&str],
) -> String {
    let modes = toml_string_array(modes);
    let conflicts_line = if conflicts.is_empty() {
        String::new()
    } else {
        format!("conflicts = {}\n", toml_string_array(conflicts))
    };
    format!(
        r#"[component]
name = "{component}"
version = "{version}"
{conflicts_line}
[component.layout]
modes = {modes}

[[component.layout.files]]
source = "bin/{component}"
target = "{{bindir}}/{component}"
mode = "0755"
type = "executable"
"#
    )
}

pub fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut tar = Builder::new(enc);
    for (path, data) in entries {
        let mut header = Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, *path, *data)
            .expect("append tar entry");
    }
    let enc = tar.into_inner().expect("finish tar");
    enc.finish().expect("finish gzip")
}

pub fn build_component_artifact(component: &str, version: &str, modes: &[&str]) -> Vec<u8> {
    let manifest = component_manifest_toml(component, version, modes);
    let bin_path = format!("bin/{component}");
    let payload = format!("#!/bin/sh\necho {component}\n");
    build_tar_gz(&[
        (".anolisa/component.toml", manifest.as_bytes()),
        (bin_path.as_str(), payload.as_bytes()),
    ])
}

pub fn adapter_manifest(framework: &str, source: Option<&str>, dest: Option<&str>) -> String {
    let mut toml = String::from(
        "[component]\nname = \"tokenless\"\nversion = \"0.1.0\"\n\n\
         [component.layout]\nmodes = [\"system\"]\n\n\
         [[adapters]]\n",
    );
    toml.push_str(&format!("framework = \"{framework}\"\n"));
    if let Some(s) = source {
        toml.push_str(&format!("source = \"{s}\"\n"));
    }
    if let Some(d) = dest {
        toml.push_str(&format!("dest = \"{d}\"\n"));
    }
    toml
}

pub fn capability_manifest(path: Option<&str>, caps: &[&str], optional: bool) -> String {
    let mut toml = String::from(
        "[component]\nname = \"agentsight\"\nversion = \"0.1.0\"\n\n\
         [component.layout]\nmodes = [\"system\"]\n\n\
         [[component.layout.files]]\n\
         source = \"bin/agentsight\"\ntarget = \"{bindir}/agentsight\"\n\
         mode = \"0755\"\ntype = \"executable\"\n\n\
         [[component.capabilities]]\n",
    );
    if let Some(p) = path {
        toml.push_str(&format!("path = \"{p}\"\n"));
    }
    toml.push_str(&format!("caps = {}\n", toml_string_array(caps)));
    if optional {
        toml.push_str("optional = true\n");
    }
    toml
}

pub fn write_empty_repo(root: &Path) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");
    std::fs::write(
        v1.join("index.toml"),
        r#"schema_version = 1
channel = "stable"
publisher = "test"
"#,
    )
    .expect("write index");
    format!("file://{}", v1.display())
}

pub fn write_local_repo(root: &Path) -> String {
    write_local_repo_component(root, "agentsight", "0.2.0", &["system"])
}

pub fn write_local_repo_component(
    root: &Path,
    component: &str,
    version: &str,
    modes: &[&str],
) -> String {
    write_local_repo_component_with_modes(root, component, version, modes, modes)
}

pub fn write_local_repo_component_with_modes(
    root: &Path,
    component: &str,
    version: &str,
    index_modes: &[&str],
    manifest_modes: &[&str],
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let artifact = build_component_artifact(component, version, manifest_modes);
    let artifact_name = format!("{component}.tar.gz");
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes = toml_string_array(index_modes);

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

pub fn write_published_layout_repo_with_meta(
    root: &Path,
    component: &str,
    version: &str,
    modes: &[&str],
) -> String {
    let env = anolisa_env::EnvService::detect();
    let version_dir = root.join("v1").join(component).join(version);
    let artifact_dir = version_dir.join(&env.os).join(&env.arch);
    std::fs::create_dir_all(&artifact_dir).expect("create artifact dirs");

    let manifest = component_manifest_toml(component, version, modes);
    std::fs::write(version_dir.join("meta.toml"), &manifest).expect("write meta");

    let artifact = build_component_artifact(component, version, modes);
    let artifact_name = format!(
        "{component}-{version}-{os}-{arch}.tar.gz",
        os = env.os,
        arch = env.arch
    );
    std::fs::write(artifact_dir.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes = toml_string_array(modes);
    let url = format!(
        "{component}/{version}/{os}/{arch}/{artifact_name}",
        os = env.os,
        arch = env.arch
    );

    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{url}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(root.join("v1/index.toml"), index).expect("write index");
    format!("file://{}", root.join("v1").display())
}

pub fn write_binary_repo_component(
    root: &Path,
    component: &str,
    version: &str,
    modes: &[&str],
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let artifact = format!("#!/bin/sh\necho {component}\n").into_bytes();
    let artifact_name = component.to_string();
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes = toml_string_array(modes);

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

pub fn write_conventional_repo(root: &Path) -> String {
    let env = anolisa_env::EnvService::detect();
    let artifact_dir = root
        .join("v1/agentsight/0.2.0")
        .join(&env.os)
        .join(&env.arch);
    std::fs::create_dir_all(&artifact_dir).expect("create repo dirs");

    let artifact = build_component_artifact("agentsight", "0.2.0", &["system"]);
    let file_name = format!("agentsight-0.2.0-{}-{}.tar.gz", env.os, env.arch);
    std::fs::write(artifact_dir.join(file_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));

    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(root.join("v1/index.toml"), index).expect("write index");
    format!("file://{}", root.join("v1").display())
}

pub fn component_manifest_toml_with_capability(
    component: &str,
    version: &str,
    modes: &[&str],
    cap_path: &str,
    caps: &[&str],
    optional: bool,
) -> String {
    let mut toml = component_manifest_toml(component, version, modes);
    toml.push_str("\n[[component.capabilities]]\n");
    toml.push_str(&format!("path = \"{cap_path}\"\n"));
    toml.push_str(&format!("caps = {}\n", toml_string_array(caps)));
    if optional {
        toml.push_str("optional = true\n");
    }
    toml
}

pub fn build_component_artifact_with_capability(
    component: &str,
    version: &str,
    modes: &[&str],
    cap_path: &str,
    caps: &[&str],
    optional: bool,
) -> Vec<u8> {
    let manifest = component_manifest_toml_with_capability(
        component, version, modes, cap_path, caps, optional,
    );
    let bin_path = format!("bin/{component}");
    let payload = format!("#!/bin/sh\necho {component}\n");
    build_tar_gz(&[
        (".anolisa/component.toml", manifest.as_bytes()),
        (bin_path.as_str(), payload.as_bytes()),
    ])
}

pub fn write_local_repo_component_with_capability(
    root: &Path,
    component: &str,
    version: &str,
    modes: &[&str],
    cap_path: &str,
    caps: &[&str],
    optional: bool,
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let artifact = build_component_artifact_with_capability(
        component, version, modes, cap_path, caps, optional,
    );
    let artifact_name = format!("{component}.tar.gz");
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes_arr = toml_string_array(modes);

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes_arr}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

pub fn service_manifest(unit: &str, enable: bool, start: bool, instance: Option<&str>) -> String {
    let mut toml = component_manifest_toml("agentsight", "0.2.0", &["system"]);
    toml.push_str("\n[[component.services]]\n");
    toml.push_str(&format!(
        "unit = \"{unit}\"\nenable = {enable}\nstart = {start}\n"
    ));
    if let Some(i) = instance {
        toml.push_str(&format!("instance = \"{i}\"\n"));
    }
    toml
}

pub fn hooks_manifest(specs: &[(&str, &str, bool)]) -> String {
    let mut toml = component_manifest_toml("demo", "0.1.0", &["system"]);
    for (phase, script, strict) in specs {
        toml.push_str("\n[[component.hooks]]\n");
        toml.push_str(&format!(
            "phase = \"{phase}\"\nscript = \"{script}\"\nstrict = {strict}\n"
        ));
    }
    toml
}

pub fn build_component_artifact_with_service(
    component: &str,
    version: &str,
    modes: &[&str],
    unit: &str,
    enable: bool,
    start: bool,
) -> Vec<u8> {
    let mut manifest = component_manifest_toml(component, version, modes);
    manifest.push_str("\n[[component.services]]\n");
    manifest.push_str(&format!(
        "unit = \"{unit}\"\nenable = {enable}\nstart = {start}\n"
    ));
    let bin_path = format!("bin/{component}");
    let payload = format!("#!/bin/sh\necho {component}\n");
    build_tar_gz(&[
        (".anolisa/component.toml", manifest.as_bytes()),
        (bin_path.as_str(), payload.as_bytes()),
    ])
}

pub fn write_local_repo_component_with_service(
    root: &Path,
    component: &str,
    version: &str,
    modes: &[&str],
    unit: &str,
    enable: bool,
    start: bool,
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let artifact =
        build_component_artifact_with_service(component, version, modes, unit, enable, start);
    let artifact_name = format!("{component}.tar.gz");
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));
    let modes_arr = toml_string_array(modes);

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = {modes_arr}
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

pub fn write_local_repo_component_with_hook(
    root: &Path,
    component: &str,
    version: &str,
    phase: &str,
    strict: bool,
    script_body: &str,
) -> String {
    let v1 = root.join("v1");
    std::fs::create_dir_all(&v1).expect("create repo dirs");

    let script_rel = format!("hooks/{component}/{}.sh", phase.replace('_', "-"));
    let mut manifest = component_manifest_toml(component, version, &["system"]);
    // Hook script is itself a laid-down layout file, mode 0755 so the
    // runner can spawn it.
    manifest.push_str("\n[[component.layout.files]]\n");
    manifest.push_str(&format!(
        "source = \"hook.sh\"\ntarget = \"{{datadir}}/{script_rel}\"\nmode = \"0755\"\n"
    ));
    manifest.push_str("\n[[component.hooks]]\n");
    manifest.push_str(&format!(
        "phase = \"{phase}\"\nscript = \"{{datadir}}/{script_rel}\"\nstrict = {strict}\n"
    ));

    let bin_path = format!("bin/{component}");
    let bin_payload = format!("#!/bin/sh\necho {component}\n");
    let artifact = build_tar_gz(&[
        (".anolisa/component.toml", manifest.as_bytes()),
        (bin_path.as_str(), bin_payload.as_bytes()),
        ("hook.sh", script_body.as_bytes()),
    ]);
    let artifact_name = format!("{component}.tar.gz");
    std::fs::write(v1.join(&artifact_name), &artifact).expect("write artifact");
    let sha = format!("{:x}", Sha256::digest(&artifact));

    let env = anolisa_env::EnvService::detect();
    let index = format!(
        r#"schema_version = 1
channel = "stable"
publisher = "test"

[[entries]]
component = "{component}"
version = "{version}"
channel = "stable"
artifact_type = "tar_gz"
backend = "raw"
url = "{artifact_name}"
os = "{os}"
arch = "{arch}"
install_modes = ["system"]
sha256 = "{sha}"
"#,
        os = env.os,
        arch = env.arch,
    );
    std::fs::write(v1.join("index.toml"), index).expect("write index");
    format!("file://{}", v1.display())
}

pub struct NoTxn;

impl PackageTransaction for NoTxn {
    fn install(&self, _package: &str) -> Result<(), PackageTransactionError> {
        panic!("adopt-path test reached a delegated dnf install");
    }
    fn update(&self, _package: &str) -> Result<(), PackageTransactionError> {
        panic!("adopt-path test reached a dnf update");
    }
    fn remove(&self, _package: &str) -> Result<(), PackageTransactionError> {
        panic!("adopt-path test reached a dnf remove");
    }
}

pub fn handle_one_with_query(
    component: String,
    args: InstallArgs,
    ctx: &CliContext,
    query: &dyn PackageQuery,
) -> Result<InstallOutcome, CliError> {
    let txn = NoTxn;
    let exec = RpmExec {
        query,
        txn: &txn,
        is_root: false,
    };
    handle_one_with_exec(component, args, ctx, &exec)
}

pub struct FakeInstaller {
    pub package: String,
    /// PackageInfo rpmdb reports after a successful install.
    pub installs_to: PackageInfo,
    pub origin: Option<String>,
    pub available: Vec<PackageInfo>,
    /// `false` makes the dnf install transaction fail.
    pub install_succeeds: bool,
    pub installed: RefCell<Option<PackageInfo>>,
    pub install_calls: Cell<usize>,
}

impl FakeInstaller {
    pub fn new(package: &str, installs_to: PackageInfo) -> Self {
        Self {
            package: package.to_string(),
            installs_to,
            origin: None,
            available: Vec::new(),
            install_succeeds: true,
            installed: RefCell::new(None),
            install_calls: Cell::new(0),
        }
    }
    pub fn with_origin(mut self, repo: &str) -> Self {
        self.origin = Some(repo.to_string());
        self
    }
    pub fn failing_install(mut self) -> Self {
        self.install_succeeds = false;
        self
    }

    fn component_capability(&self) -> String {
        rpm_component_provide(&self.package)
    }
}

impl PackageQuery for FakeInstaller {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        if package != self.package {
            return Ok(None);
        }
        Ok(self.installed.borrow().clone())
    }

    fn query_available(&self, package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        if package != self.package {
            return Ok(Vec::new());
        }
        Ok(self.available.clone())
    }

    fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
        if package != self.package {
            return Ok(None);
        }
        Ok(self.origin.clone())
    }

    fn what_provides_installed(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        if capability == self.component_capability() && self.installed.borrow().is_some() {
            Ok(vec![self.package.clone()])
        } else {
            Ok(Vec::new())
        }
    }

    fn what_provides_available(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        if capability == self.component_capability() {
            Ok(vec![self.package.clone()])
        } else {
            Ok(Vec::new())
        }
    }

    fn provided_capabilities_installed(
        &self,
        package: &str,
    ) -> Result<Vec<String>, PackageQueryError> {
        if package == self.package && self.installed.borrow().is_some() {
            Ok(vec![self.component_capability()])
        } else {
            Ok(Vec::new())
        }
    }

    fn provided_capabilities_available(
        &self,
        package: &str,
    ) -> Result<Vec<String>, PackageQueryError> {
        if package == self.package {
            Ok(vec![self.component_capability()])
        } else {
            Ok(Vec::new())
        }
    }
}

impl PackageTransaction for FakeInstaller {
    fn install(&self, package: &str) -> Result<(), PackageTransactionError> {
        self.install_calls.set(self.install_calls.get() + 1);
        assert_eq!(package, self.package, "install targeted the wrong package");
        if !self.install_succeeds {
            return Err(PackageTransactionError::TransactionFailed {
                command: "dnf".to_string(),
                operation: "install".to_string(),
                code: Some(1),
                stderr: "No match for argument".to_string(),
            });
        }
        // rpmdb now holds the package, modelling dnf placing it.
        *self.installed.borrow_mut() = Some(self.installs_to.clone());
        Ok(())
    }
    fn update(&self, _package: &str) -> Result<(), PackageTransactionError> {
        panic!("delegated-install test must not run a dnf update");
    }
    fn remove(&self, _package: &str) -> Result<(), PackageTransactionError> {
        panic!("delegated-install test must not run a dnf remove");
    }
}

#[derive(Default)]
pub struct FakeQuery {
    pub installed: Vec<(String, PackageInfo)>,
    pub origins: Vec<(String, String)>,
    pub provides: Vec<(String, Vec<String>)>,
    pub available_provides: Vec<(String, Vec<String>)>,
    pub package_provides: Vec<(String, Vec<String>)>,
    pub available_package_provides: Vec<(String, Vec<String>)>,
    pub multi_version: Vec<String>,
    pub origin_fails: bool,
    /// Simulate a host with no rpm/dnf: every rpmdb-touching query returns
    /// [`PackageQueryError::CommandMissing`], exercising the probe's
    /// warn-and-exit guard.
    pub command_missing: bool,
}

impl PackageQuery for FakeQuery {
    fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "rpm".to_string(),
            });
        }
        if self.multi_version.iter().any(|p| p == package) {
            return Err(PackageQueryError::UnexpectedOutput {
                command: "rpm".to_string(),
                detail: "2 installed versions".to_string(),
            });
        }
        Ok(self
            .installed
            .iter()
            .find(|(n, _)| n == package)
            .map(|(_, info)| info.clone()))
    }

    fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
        Ok(Vec::new())
    }

    fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
        if self.origin_fails {
            return Err(PackageQueryError::QueryFailed {
                command: "dnf".to_string(),
                code: Some(1),
                stderr: "boom".to_string(),
            });
        }
        Ok(self
            .origins
            .iter()
            .find(|(n, _)| n == package)
            .map(|(_, repo)| repo.clone()))
    }

    fn what_provides_installed(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "rpm".to_string(),
            });
        }
        Ok(self
            .provides
            .iter()
            .find(|(cap, _)| cap == capability)
            .map(|(_, names)| names.clone())
            .unwrap_or_default())
    }

    fn what_provides_available(&self, capability: &str) -> Result<Vec<String>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "dnf".to_string(),
            });
        }
        Ok(self
            .available_provides
            .iter()
            .find(|(cap, _)| cap == capability)
            .map(|(_, names)| names.clone())
            .unwrap_or_default())
    }

    fn provided_capabilities_installed(
        &self,
        package: &str,
    ) -> Result<Vec<String>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "rpm".to_string(),
            });
        }
        Ok(self
            .package_provides
            .iter()
            .find(|(pkg, _)| pkg == package)
            .map(|(_, capabilities)| capabilities.clone())
            .or_else(|| {
                self.installed
                    .iter()
                    .any(|(name, _)| name == package)
                    .then(|| vec![rpm_component_provide(package)])
            })
            .unwrap_or_default())
    }

    fn provided_capabilities_available(
        &self,
        package: &str,
    ) -> Result<Vec<String>, PackageQueryError> {
        if self.command_missing {
            return Err(PackageQueryError::CommandMissing {
                command: "dnf".to_string(),
            });
        }
        Ok(self
            .available_package_provides
            .iter()
            .find(|(pkg, _)| pkg == package)
            .map(|(_, capabilities)| capabilities.clone())
            .unwrap_or_default())
    }
}

pub fn pkg_info(name: &str, version: &str, release: Option<&str>, arch: &str) -> PackageInfo {
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

pub fn system_ctx_with_raw_repo(dry_run: bool) -> (tempfile::TempDir, CliContext) {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        "schema_version = 1\ndefault_backend = \"raw\"\n\n[backends.raw]\nbase_url = \"https://example.com/anolisa\"\n",
    )
    .expect("write repo.toml");
    let mut ctx = ctx_with_prefix(false, Some(prefix));
    ctx.dry_run = dry_run;
    (tmp, ctx)
}

pub fn system_ctx_with_configured_rpm_repo(dry_run: bool) -> (tempfile::TempDir, CliContext) {
    let tmp = tempdir().expect("tmpdir");
    let prefix = tmp.path().to_path_buf();
    let layout = FsLayout::system(Some(prefix.clone()));
    std::fs::create_dir_all(&layout.etc_dir).expect("etc dir");
    std::fs::create_dir_all(&layout.state_dir).expect("state dir");
    std::fs::write(
        layout.etc_dir.join("repo.toml"),
        r#"schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "https://example.com/anolisa"

[backends.rpm]
base_url = "https://repo.example/anolisa"
gpgcheck = false
"#,
    )
    .expect("write repo.toml");
    let mut ctx = ctx_with_prefix(false, Some(prefix));
    ctx.dry_run = dry_run;
    (tmp, ctx)
}

pub fn load_state(ctx: &CliContext) -> InstalledState {
    let layout = common::resolve_layout(ctx);
    InstalledState::load(&layout.state_dir.join("installed.toml")).expect("load state")
}

pub fn repo_with_rpm_map(pairs: &[(&str, &str)]) -> RepoConfig {
    let mut map = String::new();
    for (k, v) in pairs {
        map.push_str(&format!("{k} = \"{v}\"\n"));
    }
    RepoConfig::from_toml_str(&format!(
        "schema_version = 1\ndefault_backend = \"rpm\"\n[backends.rpm]\nbase_url = \"https://e/x\"\n[backends.rpm.package_map]\n{map}"
    ))
    .expect("parse repo")
}

pub fn linux_env() -> anolisa_env::EnvFacts {
    anolisa_env::EnvFacts {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        libc: Some("glibc".to_string()),
        kernel: Some("5.10.0".to_string()),
        pkg_base: Some("alinux4".to_string()),
        os_id: Some("alinux".to_string()),
        os_version: Some("4".to_string()),
        btf: Some(true),
        cap_bpf: Some(true),
        container: None,
        user: "root".to_string(),
        uid: 0,
        home: PathBuf::from("/root"),
    }
}

pub fn available_component_provider(component: &str, package: &str) -> (String, Vec<String>) {
    (rpm_component_provide(component), vec![package.to_string()])
}

pub fn package_component_provide(package: &str, component: &str) -> (String, Vec<String>) {
    (
        package.to_string(),
        vec![
            format!("{package} = 1.0.0"),
            rpm_component_provide(component),
        ],
    )
}

pub fn target(component: &str, package: &str) -> RpmTarget {
    RpmTarget::new(component, package)
}

pub fn situation_label(s: &RpmSituation) -> &'static str {
    match s {
        RpmSituation::Adoptable { .. } => "Adoptable",
        RpmSituation::Absent { .. } => "Absent",
        RpmSituation::NotAnolisaComponent => "NotAnolisaComponent",
        RpmSituation::Ambiguous(_) => "Ambiguous",
        RpmSituation::MultiVersion(_) => "MultiVersion",
    }
}

pub fn seed_datadir_contract(layout: &FsLayout, component: &str, toml: &str) {
    let dir = layout.datadir.join("components").join(component);
    std::fs::create_dir_all(&dir).expect("create datadir component dir");
    std::fs::write(dir.join("component.toml"), toml).expect("write datadir contract");
}

mod contract;
mod raw_e2e;
mod rpm;
