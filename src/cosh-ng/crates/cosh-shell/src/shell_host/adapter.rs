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
        config: &ShellHostConfig,
    ) {
        if config.native_mode {
            command.args(["--rcfile"]).arg(marker_path).arg("-i");
        } else {
            command
                .args(["--noprofile", "--rcfile"])
                .arg(marker_path)
                .arg("-i");
        }
        if config.login_shell {
            command.env("COSH_LOGIN_SHELL", "1");
        }
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
        if config.native_mode {
            if let Ok(original) = std::env::var("ZDOTDIR") {
                command.env("COSH_ZDOTDIR_ORIG", original);
            } else if let Ok(home) = std::env::var("HOME") {
                command.env("COSH_ZDOTDIR_ORIG", home);
            }
        }
        if config.login_shell {
            command.env("COSH_LOGIN_SHELL", "1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn test_config(native_mode: bool, login_shell: bool) -> ShellHostConfig {
        let mut config = ShellHostConfig::new("test", PathBuf::from("/tmp/test"));
        config.native_mode = native_mode;
        config.login_shell = login_shell;
        config
    }

    fn collect_args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn has_env(cmd: &Command, key: &str) -> bool {
        cmd.get_envs().any(|(k, _)| k == OsStr::new(key))
    }

    #[test]
    fn bash_native_mode_omits_noprofile() {
        let config = test_config(true, false);
        let mut cmd = Command::new("bash");
        let marker = PathBuf::from("/tmp/marker.bash");
        BashAdapter.configure_command(&mut cmd, &marker, &config);
        let args = collect_args(&cmd);
        assert!(!args.contains(&"--noprofile".to_string()));
        assert!(args.contains(&"--rcfile".to_string()));
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn bash_isolated_mode_includes_noprofile() {
        let config = test_config(false, false);
        let mut cmd = Command::new("bash");
        let marker = PathBuf::from("/tmp/marker.bash");
        BashAdapter.configure_command(&mut cmd, &marker, &config);
        let args = collect_args(&cmd);
        assert!(args.contains(&"--noprofile".to_string()));
        assert!(args.contains(&"--rcfile".to_string()));
    }

    #[test]
    fn bash_login_shell_sets_env() {
        let config = test_config(true, true);
        let mut cmd = Command::new("bash");
        let marker = PathBuf::from("/tmp/marker.bash");
        BashAdapter.configure_command(&mut cmd, &marker, &config);
        assert!(has_env(&cmd, "COSH_LOGIN_SHELL"));
    }

    #[test]
    fn bash_non_login_shell_no_env() {
        let config = test_config(true, false);
        let mut cmd = Command::new("bash");
        let marker = PathBuf::from("/tmp/marker.bash");
        BashAdapter.configure_command(&mut cmd, &marker, &config);
        assert!(!has_env(&cmd, "COSH_LOGIN_SHELL"));
    }

    #[test]
    fn zsh_login_shell_sets_env() {
        let config = test_config(true, true);
        let mut cmd = Command::new("zsh");
        let marker = PathBuf::from("/tmp/marker.zsh");
        ZshAdapter.configure_command(&mut cmd, &marker, &config);
        assert!(has_env(&cmd, "COSH_LOGIN_SHELL"));
    }

    #[test]
    fn zsh_native_mode_sets_zdotdir_orig() {
        let config = test_config(true, false);
        let mut cmd = Command::new("zsh");
        let marker = PathBuf::from("/tmp/marker.zsh");
        ZshAdapter.configure_command(&mut cmd, &marker, &config);
        assert!(has_env(&cmd, "COSH_ZDOTDIR_ORIG"));
    }

    #[test]
    fn zsh_isolated_mode_no_zdotdir_orig() {
        let config = test_config(false, false);
        let mut cmd = Command::new("zsh");
        let marker = PathBuf::from("/tmp/marker.zsh");
        ZshAdapter.configure_command(&mut cmd, &marker, &config);
        assert!(!has_env(&cmd, "COSH_ZDOTDIR_ORIG"));
    }
}
