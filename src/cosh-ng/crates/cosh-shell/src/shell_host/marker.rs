// Owner: shell_host. Bash/zsh marker scripts live in per-shell owner files
// (marker/bash.rs, marker/zsh.rs) under the registered split plan; this hub
// keeps the single shell_host::marker module path so marker and
// attempt-generation changes remain atomic.
mod bash;
mod zsh;

pub(super) use bash::bash_marker_script;
pub(super) use zsh::zsh_marker_script;
