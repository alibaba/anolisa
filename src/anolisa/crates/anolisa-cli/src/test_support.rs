//! Isolated filesystem and runtime inputs shared by CLI unit tests.

use std::path::{Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;

use crate::context::{CliContext, InstallMode, ResolvedLayouts};
use crate::packaged::PackagedDataProbe;

/// Output and execution flags for an isolated test context.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TestContextOptions {
    pub(crate) json: bool,
    pub(crate) dry_run: bool,
    pub(crate) verbose: bool,
    pub(crate) quiet: bool,
    pub(crate) no_color: bool,
}

impl Default for TestContextOptions {
    fn default() -> Self {
        Self {
            json: false,
            dry_run: false,
            verbose: false,
            quiet: true,
            no_color: true,
        }
    }
}

/// Owns an isolated user layout, system layout, repository root, and fake bin.
pub(crate) struct TestSandbox {
    tmp: tempfile::TempDir,
    user_layout: FsLayout,
    system_layout: FsLayout,
    repo_root: PathBuf,
    fake_bin: PathBuf,
}

impl TestSandbox {
    /// Create a sandbox whose writable paths are all below one temporary root.
    pub(crate) fn new() -> Self {
        let tmp = tempfile::tempdir().expect("test sandbox");
        let root = tmp.path();
        let user_layout = isolated_user_layout(root);
        let system_layout = FsLayout::system(Some(root.join("system")));
        assert_layout_contained(&user_layout, root);
        assert_layout_contained(&system_layout, root);
        Self {
            repo_root: root.join("repo"),
            fake_bin: root.join("fake-bin"),
            tmp,
            user_layout,
            system_layout,
        }
    }

    /// Temporary root retained for the sandbox lifetime.
    pub(crate) fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// Local repository root available to tests.
    pub(crate) fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Directory where tests may install fake executables.
    pub(crate) fn fake_bin(&self) -> &Path {
        &self.fake_bin
    }

    /// Build a context with default quiet, colorless test output.
    pub(crate) fn context(&self, install_mode: InstallMode) -> CliContext {
        self.context_with(install_mode, TestContextOptions::default())
    }

    /// Build a context with explicit output and execution flags.
    pub(crate) fn context_with(
        &self,
        install_mode: InstallMode,
        options: TestContextOptions,
    ) -> CliContext {
        let prefix = Some(self.system_layout.prefix.clone());
        let writable = match install_mode {
            InstallMode::System => self.system_layout.clone(),
            InstallMode::User => self.user_layout.clone(),
        };
        CliContext::from_resolved(
            install_mode,
            prefix,
            options.json,
            options.dry_run,
            options.verbose,
            options.quiet,
            options.no_color,
            ResolvedLayouts::new(writable, self.system_layout.clone()),
            PackagedDataProbe::from_inputs(None, None),
        )
    }
}

/// Build an isolated context around a temporary root owned by the caller.
pub(crate) fn context_for_root(
    root: &Path,
    install_mode: InstallMode,
    cli_prefix: Option<PathBuf>,
    options: TestContextOptions,
) -> CliContext {
    let system_root = cli_prefix
        .as_ref()
        .filter(|prefix| prefix.is_absolute() && prefix.starts_with(root))
        .cloned()
        .unwrap_or_else(|| root.join("system"));
    let system_layout = FsLayout::system(Some(system_root));
    let writable = match install_mode {
        InstallMode::System => system_layout.clone(),
        InstallMode::User => isolated_user_layout(root),
    };
    assert_layout_contained(&writable, root);
    assert_layout_contained(&system_layout, root);
    CliContext::from_resolved(
        install_mode,
        cli_prefix,
        options.json,
        options.dry_run,
        options.verbose,
        options.quiet,
        options.no_color,
        ResolvedLayouts::new(writable, system_layout),
        PackagedDataProbe::from_inputs(None, None),
    )
}

fn isolated_user_layout(root: &Path) -> FsLayout {
    FsLayout::user_with_overrides(
        root.join("home"),
        Some(root.join("xdg-data")),
        Some(root.join("xdg-config")),
        Some(root.join("xdg-state")),
        Some(root.join("xdg-cache")),
        Some(root.join("xdg-runtime")),
    )
}

fn assert_layout_contained(layout: &FsLayout, root: &Path) {
    for path in [
        &layout.bin_dir,
        &layout.lib_dir,
        &layout.libexec_dir,
        &layout.datadir,
        &layout.etc_dir,
        &layout.state_dir,
        &layout.cache_dir,
        &layout.log_dir,
        &layout.backup_dir,
        &layout.runtime_dir,
        &layout.systemd_unit_dir,
        &layout.systemd_user_unit_dir,
    ] {
        assert!(
            path.starts_with(root),
            "test layout path {} escapes sandbox {}",
            path.display(),
            root.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_keep_writable_and_visible_system_layouts_contained() {
        let sandbox = TestSandbox::new();
        assert!(sandbox.repo_root().starts_with(sandbox.root()));
        assert!(sandbox.fake_bin().starts_with(sandbox.root()));

        for mode in [InstallMode::User, InstallMode::System] {
            let ctx = sandbox.context(mode);
            assert_layout_contained(ctx.layout(), sandbox.root());
            assert_layout_contained(ctx.visible_system_layout(), sandbox.root());
        }
    }

    #[test]
    fn user_prefix_only_selects_the_visible_system_layout() {
        let sandbox = TestSandbox::new();
        let ctx = sandbox.context(InstallMode::User);

        assert!(
            ctx.layout()
                .etc_dir
                .starts_with(sandbox.root().join("xdg-config"))
        );
        assert!(
            ctx.visible_system_layout()
                .etc_dir
                .starts_with(sandbox.root().join("system"))
        );
        assert_ne!(ctx.layout().prefix, ctx.visible_system_layout().prefix);
    }
}
