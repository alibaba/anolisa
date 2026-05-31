use libbpf_cargo::SkeletonBuilder;
use std::env;
use std::path::PathBuf;

fn generate_skeleton(out: &mut PathBuf, name: &str) {
    let c_path = format!("src/bpf/{name}.bpf.c");
    let rs_name = format!("{name}.skel.rs");
    out.push(&rs_name);

    SkeletonBuilder::new()
        .source(&c_path)
        .build_and_generate(&out)
        .unwrap();

    out.pop();
    println!("cargo:rerun-if-changed={c_path}");
}

fn generate_header(out: &mut PathBuf, name: &str) {
    let header_path = format!("src/bpf/{name}.h");
    let rs_name = format!("{name}.rs");

    out.push(&rs_name);
    let bindings = bindgen::Builder::default()
        .header(&header_path)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .unwrap();
    bindings.write_to_file(&out).unwrap();
    out.pop();

    println!("cargo:rerun-if-changed={header_path}");
}

fn main() {
    let mut out =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set in build script"));

    generate_skeleton(&mut out, "sslsniff");
    generate_header(&mut out, "sslsniff");
    
    // Generate proctrace skeleton and bindings
    generate_skeleton(&mut out, "proctrace");
    generate_header(&mut out, "proctrace");
    
    // Generate procmon skeleton and bindings
    generate_skeleton(&mut out, "procmon");
    generate_header(&mut out, "procmon");

    // Generate filewatch skeleton and bindings
    generate_skeleton(&mut out, "filewatch");
    generate_header(&mut out, "filewatch");

    // Generate filewrite skeleton and bindings
    generate_skeleton(&mut out, "filewrite");
    generate_header(&mut out, "filewrite");

    // Generate udpdns skeleton and bindings
    generate_skeleton(&mut out, "udpdns");
    generate_header(&mut out, "udpdns");

    // Generate tcpsniff skeleton (no header — reuses sslsniff.h event format)
    generate_skeleton(&mut out, "tcpsniff");
    
    // generate_header(&mut out, "frametypes");
    // generate_header(&mut out, "errors");
    // generate_header(&mut out, "stackdeltatypes");

    // 确保 frontend-dist 目录存在，避免 include_dir! 宏在编译期因目录不存在而 panic
    // 前端构建产物放入该目录后重新编译即可嵌入
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let frontend_dist = PathBuf::from(&manifest_dir).join("frontend-dist");
    if !frontend_dist.exists() {
        std::fs::create_dir_all(&frontend_dist)
            .expect("Failed to create frontend-dist directory");
    }
    // Watch the directory AND each file inside it so cargo detects content changes
    println!("cargo:rerun-if-changed=frontend-dist");
    if let Ok(entries) = std::fs::read_dir(&frontend_dist) {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }

    // Generate C header from src/ffi.rs via cbindgen
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let header_path = PathBuf::from(&crate_dir).join("include").join("agentsight.h");
    std::fs::create_dir_all(header_path.parent().unwrap())
        .expect("Failed to create include/ directory");
    cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(
            cbindgen::Config::from_file(PathBuf::from(&crate_dir).join("cbindgen.toml"))
                .expect("Failed to read cbindgen.toml"),
        )
        .with_parse_exclude(&["skill_metrics".to_string()])
        .generate()
        .expect("cbindgen failed to generate C header")
        .write_to_file(&header_path);
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    // Drift guard: cbindgen 0.27 silently skips `#[unsafe(no_mangle)]`, so the
    // function declarations in cbindgen.toml are hand-maintained. Compare the
    // export NAMES on both sides and fail the build if they disagree.
    //
    // NOTE: this guard checks NAMES only — it does NOT verify return types or
    // parameter signatures. A maintainer changing only a return type or arg
    // list (no name change) will pass this check but still produce an ABI
    // mismatch. Keep cbindgen.toml's hand-written signatures in lockstep with
    // src/ffi.rs by hand until cbindgen learns the new attribute.
    check_ffi_header_drift(&crate_dir, &header_path);
}

/// Extract every `agentsight_<ident>` that appears as a function declaration
/// `<...> agentsight_<ident>(` in the generated header. Skips doc comments,
/// `#define`, `#include`, `typedef`, and multi-line parameter continuations
/// by only considering lines that begin with an identifier character.
fn extract_header_decl_names(header: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for l in header.lines() {
        let first_is_ident = l
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false);
        if !first_is_ident || l.starts_with("typedef") {
            continue;
        }
        let bytes = l.as_bytes();
        let mut i = 0;
        while i + "agentsight_".len() < bytes.len() {
            if l[i..].starts_with("agentsight_") {
                let mut j = i + "agentsight_".len();
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'(' {
                    out.insert(l[i..j].to_string());
                    break; // one declaration per line
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Extract every `pub [unsafe] extern "C" fn agentsight_<ident>` from src/ffi.rs.
/// We match the function name directly (no need to find the matching
/// `#[unsafe(no_mangle)]` attribute first — every export uses that signature
/// shape, and a stray `agentsight_*` identifier elsewhere in the file would
/// not be preceded by `extern "C" fn`).
fn extract_ffi_export_names(src: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for l in src.lines() {
        let t = l.trim_start();
        // Both `pub extern "C" fn agentsight_X(` and
        // `pub unsafe extern "C" fn agentsight_X(` shapes.
        let after_fn = if let Some(rest) = t.strip_prefix("pub extern \"C\" fn ") {
            rest
        } else if let Some(rest) = t.strip_prefix("pub unsafe extern \"C\" fn ") {
            rest
        } else {
            continue;
        };
        if let Some(name) = after_fn.split('(').next() {
            let name = name.trim();
            if name.starts_with("agentsight_") {
                out.insert(name.to_string());
            }
        }
    }
    out
}

fn check_ffi_header_drift(crate_dir: &str, header_path: &std::path::Path) {
    let ffi_path = PathBuf::from(crate_dir).join("src/ffi.rs");
    let ffi_src = std::fs::read_to_string(&ffi_path).unwrap_or_else(|e| {
        panic!(
            "FFI drift guard: cannot read {}: {e}. If you renamed or moved ffi.rs, \
             update build.rs::check_ffi_header_drift.",
            ffi_path.display()
        )
    });
    let header = std::fs::read_to_string(header_path).unwrap_or_else(|e| {
        panic!(
            "FFI drift guard: cannot read generated header {}: {e}. cbindgen \
             should have just written it; check earlier cbindgen errors.",
            header_path.display()
        )
    });

    let ffi_names = extract_ffi_export_names(&ffi_src);
    let header_names = extract_header_decl_names(&header);

    // Sanity check: the FFI side count should match `#[unsafe(no_mangle)]`
    // occurrences. A mismatch means our extractor missed an export shape we
    // don't know about — fail loudly rather than under-reporting.
    let marker_count = ffi_src.matches("#[unsafe(no_mangle)]").count();
    assert_eq!(
        marker_count,
        ffi_names.len(),
        "FFI drift guard: {} `#[unsafe(no_mangle)]` markers in src/ffi.rs but \
         extract_ffi_export_names found {} `pub [unsafe] extern \"C\" fn agentsight_*` \
         declarations. The extractor likely needs updating (build.rs).",
        marker_count,
        ffi_names.len()
    );

    let missing_in_header: Vec<&String> = ffi_names.difference(&header_names).collect();
    let stale_in_header: Vec<&String> = header_names.difference(&ffi_names).collect();

    if !missing_in_header.is_empty() || !stale_in_header.is_empty() {
        panic!(
            "FFI header drift detected — update the `after_includes` block in cbindgen.toml.\n\
             missing in header (declared in src/ffi.rs but absent from cbindgen.toml): {:?}\n\
             stale in header   (declared in cbindgen.toml but absent from src/ffi.rs):  {:?}\n\
             NOTE: this guard checks NAMES only; signature drift (return type, \
             parameter types/order) is NOT detected — verify by hand.",
            missing_in_header, stale_in_header,
        );
    }
}
