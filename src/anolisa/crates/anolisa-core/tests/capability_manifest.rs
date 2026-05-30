//! Smoke tests that all bundled capability manifests parse and register.

use anolisa_core::capability::{CapabilityManifest, CapabilityResolver};
use std::fs;
use std::path::PathBuf;

fn manifests_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("..");
    p.push("..");
    p.push("manifests");
    p.push("capabilities");
    p.canonicalize()
        .expect("manifests/capabilities should exist")
}

#[test]
fn all_capability_manifests_parse() {
    let dir = manifests_dir();
    let mut count = 0;
    let mut resolver = CapabilityResolver::new();

    for entry in fs::read_dir(&dir).expect("read capabilities dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let m = CapabilityManifest::from_file(&path)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
        resolver.register(m);
        count += 1;
    }

    assert_eq!(count, 9, "expected 9 capability manifests, got {count}");

    // Spot-check a few canonical capabilities resolve.
    for name in [
        "token-optimization",
        "agent-memory",
        "sandbox",
        "os-security",
    ] {
        let plan = resolver
            .resolve(name)
            .unwrap_or_else(|e| panic!("resolve {name}: {e}"));
        assert!(!plan.components.is_empty(), "{name} should have components");
    }
}
