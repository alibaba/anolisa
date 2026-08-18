// Owner: shell_host (bash marker script). Emitted protocol must stay
// byte-identical to the pre-split marker.rs; golden coverage lives in
// osc_tests.rs and tests/shell_host/marker.rs. The script body lives in
// three composable assets (each under the 700-line layout threshold,
// executing the split plan recorded in the large-file inventory when
// #2598 pushed the former inline literal past the 1000-line blocking
// bar): session init + emit helpers (core), the attempt/cnf dispatch
// chain (dispatch), and the precmd/prompt frames + top-level install
// (frames). The runtime concatenation keeps the emitted bytes identical
// to the former single literal.
pub(in crate::shell_host) fn bash_marker_script() -> &'static str {
    concat!(
        include_str!("bash_marker_core.sh"),
        include_str!("bash_marker_dispatch.sh"),
        include_str!("bash_marker_frames.sh"),
    )
}
