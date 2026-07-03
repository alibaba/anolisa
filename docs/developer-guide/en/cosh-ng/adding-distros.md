# Adding Distribution Support

## Overview

cosh-ng abstracts OS differences through the `Distro` enum. Adding support for a new distribution requires modifications at two layers: the detection layer (`cosh-platform/src/detect.rs`) and the backend routing layer (`cosh-platform/src/pkg.rs`, etc.).

## Steps

### 1. Add Distro Enum Variant

Add a variant in `crates/cosh-platform/src/detect.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distro {
    // ...existing...
    MyDistro { version: String },   // New
}
```

### 2. Implement Detection Logic

Add ID mapping in the `detect_from_content()` match branch:

```rust
match id.as_deref() {
    // ...existing...
    Some("mydistro") => Distro::MyDistro { version },
    // ...
}
```

Linux systems identify distributions by parsing the `ID` field from `/etc/os-release`. Values must match in lowercase.

### 3. Implement Helper Methods

```rust
impl Distro {
    pub fn id_str(&self) -> &str {
        match self {
            // ...existing...
            Distro::MyDistro { .. } => "mydistro",
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            // ...existing...
            Distro::MyDistro { version } => format!("MyDistro {}", version),
        }
    }

    pub fn pkg_manager(&self) -> PkgManager {
        match self {
            // ...existing...
            Distro::MyDistro { .. } => PkgManager::Dnf, // Choose based on actual situation
        }
    }
}
```

If the new distribution uses a package manager not in the existing `PkgManager` enum, extend that enum first.

### 4. Add Package Manager Backend (if needed)

If a new `PkgManager` variant is needed, add the corresponding command builder in `crates/cosh-platform/src/pkg.rs`:

```rust
// New PkgManager variant
pub enum PkgManager {
    // ...existing...
    Pacman,
}

// Add routing branch in pkg_install / pkg_remove / pkg_search / pkg_list
PkgManager::Pacman => ("pacman", vec!["-S", "--noconfirm", package]),
```

### 5. Add Unit Tests

Add in the `#[cfg(test)]` module of `detect.rs`:

```rust
#[test]
fn test_detect_mydistro() {
    let content = "NAME=\"My Distro\"\nVERSION_ID=\"1.0\"\nID=mydistro\n";
    let distro = Distro::detect_from_content(content);
    assert_eq!(distro, Distro::MyDistro { version: "1.0".into() });
    assert_eq!(distro.pkg_manager(), PkgManager::Dnf);
}
```

### 6. Run Tests to Verify

```bash
cd src/cosh-ng

# Run detection-related tests
cargo test --locked -p cosh-platform test_detect

# Run full test suite
cargo test --locked -p cosh-platform

# Run CLI integration tests (ensure new routing doesn't break JSON envelope)
cargo test --locked -p cosh-cli
```

## Current Support Matrix

| Distribution ID | Distro Variant | PkgManager | Notes |
|----------------|---------------|------------|-------|
| `alinux` | `Alinux` | Dnf | Alibaba Cloud native Linux |
| `centos` | `CentOS` | Dnf | |
| `fedora` | `Fedora` | Dnf | |
| `ubuntu` | `Ubuntu` | Apt | |
| `debian` | `Debian` | Apt | |
| `opensuse-leap` / `opensuse-tumbleweed` / `sles` | `OpenSUSE` | Zypper | Three IDs map to same variant |
| macOS (compile target) | `MacOS` | Brew | Detected via `sw_vers` |

## Design Constraints

| Rule | Description |
|------|-------------|
| Lowercase ID | `detect_from_content()` does `to_lowercase()` on ID |
| Unknown fallback | Unrecognized IDs fall into `Unknown(String)`, subsequent operations return `UnsupportedDistro` |
| Multi-ID merge | Multiple IDs can map to the same Distro variant (e.g., opensuse family) |
| Package manager decoupling | `PkgManager` and `Distro` are separate enums, mapped via `pkg_manager()` |
| No config merging | Detection logic only takes the first matching `ID` and `VERSION_ID` |

## Complete Checklist

- [ ] Add new variant to `Distro` enum
- [ ] Add match branch in `detect_from_content()`
- [ ] `id_str()` returns correct string identifier
- [ ] `display_name()` returns readable name
- [ ] `pkg_manager()` maps to correct package manager
- [ ] `Display` trait (via `display_name()`) formats correctly
- [ ] Unit tests cover normal parsing and edge cases
- [ ] If new PkgManager needed, add routing in `pkg.rs` functions
- [ ] Update user documentation `users/supported-distros.md`
