//! BPF verifier load tests — verify every eBPF program passes the kernel
//! verifier on the running kernel.
//!
//! These tests require CAP_BPF + CAP_PERFMON (or root). They are `#[ignore]`
//! by default so `cargo test` skips them; run explicitly with:
//!
//!     sudo cargo test --test bpf_load -- --ignored
//!
//! Each test names the probe it covers so a failure immediately identifies
//! which BPF program the verifier rejected.

use agentsight::probes::{
    FileWatch, FileWriteProbe, Probes, ProcMon, ProcTrace, SharedMaps, SslSniff, TcpSniff, UdpDns,
};

fn make_shared_maps() -> (ProcTrace, SharedMaps) {
    let pt = ProcTrace::new().expect("proctrace open+load");
    let shared = SharedMaps::new(pt.rb_handle().expect("rb handle")).with_traced_processes(
        pt.traced_processes_handle()
            .expect("traced_processes handle"),
    );
    (pt, shared)
}

#[test]
#[ignore]
fn proctrace_bpf_loads() {
    ProcTrace::new().expect("proctrace BPF should load on this kernel");
}

#[test]
#[ignore]
fn sslsniff_bpf_loads() {
    SslSniff::new().expect("sslsniff BPF should load on this kernel");
}

#[test]
#[ignore]
fn procmon_bpf_loads() {
    let (_pt, shared) = make_shared_maps();
    ProcMon::new_with_shared(&shared).expect("procmon BPF should load on this kernel");
}

#[test]
#[ignore]
fn filewatch_bpf_loads() {
    let (_pt, shared) = make_shared_maps();
    FileWatch::new_with_shared(&shared).expect("filewatch BPF should load on this kernel");
}

#[test]
#[ignore]
fn filewrite_bpf_loads() {
    let (_pt, shared) = make_shared_maps();
    FileWriteProbe::new_with_shared(&shared).expect("filewrite BPF should load on this kernel");
}

#[test]
#[ignore]
fn udpdns_bpf_loads() {
    let (_pt, shared) = make_shared_maps();
    UdpDns::new_with_shared(&shared).expect("udpdns BPF should load on this kernel");
}

#[test]
#[ignore]
fn tcpsniff_bpf_loads() {
    let (_pt, shared) = make_shared_maps();
    TcpSniff::new_with_shared(&shared).expect("tcpsniff BPF should load on this kernel");
}

#[test]
#[ignore]
fn all_probes_load() {
    Probes::new(&[], None, true, true, &[])
        .expect("unified Probes (all BPF programs) should load on this kernel");
}
