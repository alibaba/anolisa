//! Registry: local catalog lookup plus remote distribution-registry access.
//!
//! Two distinct concerns share this module:
//! - [`Registry`] is the historical lookup facade over the bundled [`Catalog`]
//!   (component by name). It delegates fully to `Catalog`.
//! - [`RegistryConfig`] + [`RegistryClient`] resolve the distribution
//!   `index.toml` URL/cache policy and fetch the index over HTTP with a TTL
//!   cache and offline fallback. Submodules are private; the public types are
//!   re-exported flat from here (and again from the crate root).

mod cache;
mod client;
mod config;
mod error;

pub use client::{FetchFailure, FetchedMeta, HttpFetch, IndexFreshness, RegistryClient, UreqFetch};
pub use config::RegistryConfig;
pub use error::RegistryError;

use crate::catalog::{Catalog, CatalogError, CatalogLayers};
use crate::manifest::ComponentManifest;
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

    /// Lookup a component by name.
    pub fn component(&self, name: &str) -> Option<&ComponentManifest> {
        self.catalog.component(name)
    }

    /// List all components in catalog order.
    pub fn list_components(&self) -> Vec<&ComponentManifest> {
        self.catalog.list_components()
    }
}
