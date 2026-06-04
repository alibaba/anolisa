//! Registry: thin wrapper over [`Catalog`].
//!
//! Registry is the historical entry point used by callers that just want
//! lookup-by-name semantics over the bundled catalog. It now delegates fully
//! to `Catalog` so layering and schema parsing live in one place.

use crate::catalog::{Catalog, CatalogError, CatalogLayers};
use crate::manifest::{CapabilityManifest, ComponentManifest};
use std::path::PathBuf;

/// Lookup facade over a loaded [`Catalog`].
#[derive(Debug, Clone)]
pub struct Registry {
    catalog: Catalog,
}

impl Registry {
    /// Construct a registry backed by the supplied bundled-manifests root.
    pub fn from_bundled(bundled: PathBuf) -> Result<Self, CatalogError> {
        let catalog = Catalog::load(CatalogLayers::bundled_only(bundled))?;
        Ok(Self { catalog })
    }

    /// Construct a registry from an already-built [`Catalog`].
    pub fn from_catalog(catalog: Catalog) -> Self {
        Self { catalog }
    }

    /// Borrow the underlying catalog for callers that need full layer data.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Lookup a capability by name.
    pub fn capability(&self, name: &str) -> Option<&CapabilityManifest> {
        self.catalog.capability(name)
    }

    /// Lookup a component by name.
    pub fn component(&self, name: &str) -> Option<&ComponentManifest> {
        self.catalog.component(name)
    }

    /// List all capabilities in catalog order.
    pub fn list_capabilities(&self) -> Vec<&CapabilityManifest> {
        self.catalog.list_capabilities()
    }

    /// List all components in catalog order.
    pub fn list_components(&self) -> Vec<&ComponentManifest> {
        self.catalog.list_components()
    }
}
