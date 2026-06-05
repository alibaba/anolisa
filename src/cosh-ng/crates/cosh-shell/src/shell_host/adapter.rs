use std::path::Path;
use std::process::Command;

use super::marker::{bash_marker_script, zsh_marker_script};
use super::model::ShellHostConfig;

pub(super) trait ShellAdapter {
    fn executable<'a>(&self, config: &'a ShellHostConfig) -> &'a str;
    fn marker_filename(&self) -> &'static str;
    fn marker_script(&self) -> &'static str;
    fn configure_command(
        &self,
        command: &mut Command,
        marker_path: &Path,
        config: &ShellHostConfig,
    );
}

pub(super) struct BashAdapter;

impl ShellAdapter for BashAdapter {
    fn executable<'a>(&self, config: &'a ShellHostConfig) -> &'a str {
        &config.bash_path
    }

    fn marker_filename(&self) -> &'static str {
        "cosh-marker.bash"
    }

    fn marker_script(&self) -> &'static str {
        bash_marker_script()
    }

    fn configure_command(
        &self,
        command: &mut Command,
        marker_path: &Path,
        _config: &ShellHostConfig,
    ) {
        command
            .args(["--noprofile", "--rcfile"])
            .arg(marker_path)
            .arg("-i");
    }
}

pub(super) struct ZshAdapter;

impl ShellAdapter for ZshAdapter {
    fn executable<'a>(&self, config: &'a ShellHostConfig) -> &'a str {
        &config.zsh_path
    }

    fn marker_filename(&self) -> &'static str {
        ".zshrc"
    }

    fn marker_script(&self) -> &'static str {
        zsh_marker_script()
    }

    fn configure_command(
        &self,
        command: &mut Command,
        _marker_path: &Path,
        config: &ShellHostConfig,
    ) {
        command.arg("-i").env("ZDOTDIR", &config.work_dir);
    }
}
