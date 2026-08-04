// Owner: shell_host (zsh marker script). The script body lives in
// zsh_marker.sh (include_str!) to keep this file under the 700-line
// layout threshold; the emitted protocol must stay byte-identical to the
// pre-split marker.rs. Golden coverage lives in osc_tests.rs and
// tests/shell_host/marker.rs.
const ZSH_MARKER_SCRIPT: &str = include_str!("zsh_marker.sh");

pub(in crate::shell_host) fn zsh_marker_script() -> &'static str {
    ZSH_MARKER_SCRIPT
}
