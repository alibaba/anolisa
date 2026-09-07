// Makes the prebuilt eBPF CO-RE object available to lib.rs (include_bytes! from
// OUT_DIR). By default this just copies the committed prebuilt/process.bpf.o, so
// `cargo build` / `cargo install` needs NO clang/llvm/libbpf and no submodules.
//
// Set ACTPLANE_REBUILD_BPF=1 to rebuild from the kernel C via the Makefile
// (requires the BPF toolchain + the libbpf/bpftool submodules); the freshly
// built object is also written back to prebuilt/ so it can be committed.
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let prebuilt = manifest.join("prebuilt/process.bpf.o");
    let built = manifest.join(".output/process.bpf.o");

    println!("cargo:rerun-if-env-changed=ACTPLANE_REBUILD_BPF");
    println!("cargo:rerun-if-changed={}", prebuilt.display());

    let rebuild = env::var_os("ACTPLANE_REBUILD_BPF").is_some();

    if rebuild || !prebuilt.exists() {
        for f in [
            "process.bpf.c",
            "process.h",
            "taint.h",
            "taint_engine.bpf.h",
            "capability.bpf.h",
            "channel.bpf.h",
            "Makefile",
        ] {
            println!("cargo:rerun-if-changed={}", manifest.join(f).display());
        }
        let status = Command::new("make")
            .arg("-C")
            .arg(&manifest)
            .arg("process")
            .status()
            .expect("run make -C bpf process (ACTPLANE_REBUILD_BPF)");
        assert!(status.success(), "make -C bpf process failed");
        // Refresh the committed copy so the rebuild can be committed.
        std::fs::create_dir_all(manifest.join("prebuilt")).ok();
        std::fs::copy(&built, &prebuilt)
            .unwrap_or_else(|e| panic!("copy {} -> prebuilt: {e}", built.display()));
    }

    let src = if prebuilt.exists() { &prebuilt } else { &built };
    std::fs::copy(src, out.join("process.bpf.o"))
        .unwrap_or_else(|e| panic!("copy {} -> OUT_DIR: {e}", src.display()));
}
