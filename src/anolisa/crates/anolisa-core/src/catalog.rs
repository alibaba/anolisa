//! Catalog: layered loader for component manifests.
//!
//! Three layers are supported and applied in order of increasing precedence:
//!
//! 1. `bundled` — the manifests shipped with the source tree (always present).
//! 2. `system` — `/etc/anolisa/manifests` (optional, ops overrides).
//! 3. `user`   — `~/.config/anolisa/manifests` (optional, per-user overrides).
//!
//! Within each layer the loader walks `runtime/*.toml` and `osbase/*.toml`
//! and keys entries by manifest name. Within a layer, files are sorted by
//! path; later files and later layers with the same key replace earlier
//! entries.
//!
//! `Catalog::load` is intentionally tolerant: missing layer directories are
//! ignored and individual malformed manifests surface as `CatalogError`s
//! rather than panicking.

use crate::manifest::{ComponentManifest, ManifestError, manifest_paths};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Paths for each manifest layer. Later layers override earlier ones.
#[derive(Debug, Clone)]
pub struct CatalogLayers {
    /// System override layer, usually `/etc/anolisa/manifests`.
    pub system: Option<PathBuf>,
    /// Per-user override layer.
    pub user: Option<PathBuf>,
    /// Bundled manifests shipped with the package/source tree.
    pub bundled: PathBuf,
}

impl CatalogLayers {
    /// Helper for the common case of a bundled-only catalog (used by tests
    /// and by the CLI when no overrides are configured).
    pub fn bundled_only(bundled: PathBuf) -> Self {
        Self {
            system: None,
            user: None,
            bundled,
        }
    }
}

/// Loaded component manifests with their source layers.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Component manifests keyed by component name.
    pub components: BTreeMap<String, ComponentManifest>,
    /// Layer paths used to build this catalog.
    pub layers: CatalogLayers,
}

impl Catalog {
    /// Load the catalog from disk, walking each configured layer in
    /// precedence order. A missing optional layer is silently skipped.
    pub fn load(layers: CatalogLayers) -> Result<Self, CatalogError> {
        let mut components: BTreeMap<String, ComponentManifest> = BTreeMap::new();

        let layered: [Option<&Path>; 3] = [
            Some(layers.bundled.as_path()),
            layers.system.as_deref(),
            layers.user.as_deref(),
        ];

        for layer_root in layered.into_iter().flatten() {
            load_layer(layer_root, &mut components)?;
        }

        Ok(Self { components, layers })
    }

    /// Lookup a component manifest by stable component name.
    pub fn component(&self, name: &str) -> Option<&ComponentManifest> {
        self.components.get(name)
    }

    /// Return components in deterministic key order.
    pub fn list_components(&self) -> Vec<&ComponentManifest> {
        self.components.values().collect()
    }
}

fn load_layer(
    root: &Path,
    components: &mut BTreeMap<String, ComponentManifest>,
) -> Result<(), CatalogError> {
    if !root.exists() {
        return Ok(());
    }

    for sub in ["runtime", "osbase"] {
        for path in manifest_paths(&root.join(sub)) {
            let m = ComponentManifest::from_file(&path).map_err(CatalogError::from)?;
            // Deterministic overlay: manifest_paths() is sorted, so later
            // files in this layer replace earlier files with the same name;
            // cross-layer replacement follows Catalog::load precedence
            // (bundled < system < user).
            components.insert(m.component.name.clone(), m);
        }
    }

    Ok(())
}

/// Errors raised while loading a manifest catalog.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// A manifest file failed to load or parse.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn bundled_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("..");
        p.push("..");
        p.push("manifests");
        p.canonicalize().expect("bundled manifests path resolves")
    }

    fn minimal_component_toml(name: &str, display_name: &str) -> String {
        format!(
            r#"
                [component]
                name = "{name}"
                version = "0.0.1"
                layer = "runtime"
                display_name = "{display_name}"
            "#
        )
    }

    #[test]
    fn loads_bundled_catalog() {
        let catalog = Catalog::load(CatalogLayers::bundled_only(bundled_root()))
            .expect("bundled catalog loads");
        // Spot-check a few canonical names.
        assert!(catalog.component("agentsight").is_some());
        assert!(catalog.component("tokenless").is_some());
        // Layer scan should pick up all bundled fixtures.
        assert!(
            catalog.list_components().len() >= 6,
            "expected at least 6 components, got {}",
            catalog.list_components().len()
        );
    }

    #[test]
    fn user_layer_overrides_bundled() {
        let tmp = tempdir().expect("tempdir");
        let runtime_dir = tmp.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("mkdir runtime_dir");
        fs::write(
            runtime_dir.join("agentsight.toml"),
            minimal_component_toml("agentsight", "USER LAYER OVERRIDE"),
        )
        .expect("write override");

        let layers = CatalogLayers {
            system: None,
            user: Some(tmp.path().to_path_buf()),
            bundled: bundled_root(),
        };
        let catalog = Catalog::load(layers).expect("load with override");
        let m = catalog.component("agentsight").expect("component present");
        assert_eq!(
            m.component.display_name.as_deref(),
            Some("USER LAYER OVERRIDE")
        );
    }

    #[test]
    fn lookup_roundtrip() {
        let catalog = Catalog::load(CatalogLayers::bundled_only(bundled_root()))
            .expect("bundled catalog loads");

        let comp = catalog.component("agentsight").expect("agentsight present");
        assert_eq!(comp.component.name, "agentsight");
    }

    #[test]
    fn system_layer_then_user_layer_precedence() {
        let sys = tempdir().expect("sys tempdir");
        let usr = tempdir().expect("usr tempdir");
        fs::create_dir_all(sys.path().join("runtime")).expect("mkdir sys runtime");
        fs::create_dir_all(usr.path().join("runtime")).expect("mkdir usr runtime");
        fs::write(
            sys.path().join("runtime/agent-memory.toml"),
            minimal_component_toml("agent-memory", "SYSTEM"),
        )
        .expect("write sys");
        fs::write(
            usr.path().join("runtime/agent-memory.toml"),
            minimal_component_toml("agent-memory", "USER"),
        )
        .expect("write usr");

        let layers = CatalogLayers {
            system: Some(sys.path().to_path_buf()),
            user: Some(usr.path().to_path_buf()),
            bundled: bundled_root(),
        };
        let catalog = Catalog::load(layers).expect("load layered");
        let m = catalog
            .component("agent-memory")
            .expect("agent-memory present");
        assert_eq!(m.component.display_name.as_deref(), Some("USER"));
    }

    #[test]
    fn duplicate_manifest_names_use_last_loaded_entry() {
        let tmp = tempdir().expect("tempdir");
        let runtime_dir = tmp.path().join("runtime");
        fs::create_dir_all(&runtime_dir).expect("mkdir runtime_dir");
        fs::write(
            runtime_dir.join("00-first.toml"),
            minimal_component_toml("duplicate-component", "FIRST"),
        )
        .expect("write first");
        fs::write(
            runtime_dir.join("99-last.toml"),
            minimal_component_toml("duplicate-component", "LAST"),
        )
        .expect("write last");

        let catalog = Catalog::load(CatalogLayers::bundled_only(tmp.path().to_path_buf()))
            .expect("load duplicate manifests");
        let m = catalog
            .component("duplicate-component")
            .expect("duplicate component present");
        assert_eq!(m.component.display_name.as_deref(), Some("LAST"));
    }
}
