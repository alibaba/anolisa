//! HTTP + cache front-end for the distribution index.
//!
//! [`RegistryClient::fetch_index`] resolves the index per a fixed decision
//! table over cache freshness and network reachability (design §1.6). The
//! transport is abstracted behind [`HttpFetch`] so the TTL/offline logic is
//! unit-testable without real sockets; production uses [`UreqFetch`].

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use super::cache::RegistryCache;
use super::config::RegistryConfig;
use super::error::RegistryError;
use sha2::{Digest, Sha256};

use crate::distribution::DistributionIndex;
use crate::download::DEFAULT_HTTP_READ_TIMEOUT;
use crate::manifest::ComponentManifest;

/// Connect timeout for index fetches; mirrors the downloader's policy.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How the returned index was obtained — surfaced to the CLI for warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexFreshness {
    /// Fetched from the network on this call.
    Fresh,
    /// Served from cache within TTL; no network request was made.
    CacheHit,
    /// TTL expired and the network was unreachable; a stale cached copy was
    /// served under `offline_fallback`.
    StaleOffline,
}

/// Reason an [`HttpFetch::get`] call failed.
///
/// The client treats both variants as "could not obtain a fresh index"; the
/// distinction is preserved only for diagnostics.
#[derive(Debug)]
pub enum FetchFailure {
    /// Endpoint was unreachable (DNS/connect/read transport failure).
    Network {
        /// Transport-layer diagnostic.
        reason: String,
    },
    /// Endpoint responded with a non-success HTTP status.
    Status {
        /// Numeric HTTP status code.
        code: u16,
    },
}

impl std::fmt::Display for FetchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network { reason } => write!(f, "network error: {reason}"),
            Self::Status { code } => write!(f, "http status {code}"),
        }
    }
}

/// Pluggable HTTP GET backend, injectable for tests.
pub trait HttpFetch {
    /// GET `url`, returning the response body on a success status.
    ///
    /// # Errors
    /// [`FetchFailure`] when the endpoint is unreachable or returns non-2xx.
    fn get(&self, url: &str) -> Result<Vec<u8>, FetchFailure>;
}

/// Production [`HttpFetch`] backed by `ureq`, with the same timeout policy as
/// the artifact downloader.
pub struct UreqFetch {
    connect_timeout: Duration,
    read_timeout: Duration,
}

impl Default for UreqFetch {
    fn default() -> Self {
        Self {
            connect_timeout: HTTP_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_HTTP_READ_TIMEOUT,
        }
    }
}

impl HttpFetch for UreqFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, FetchFailure> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(self.connect_timeout)
            .timeout_read(self.read_timeout)
            .build();
        let response = agent.get(url).call().map_err(|err| match err {
            ureq::Error::Status(code, _) => FetchFailure::Status { code },
            ureq::Error::Transport(transport) => FetchFailure::Network {
                reason: transport.to_string(),
            },
        })?;
        let mut buf = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| FetchFailure::Network {
                reason: e.to_string(),
            })?;
        Ok(buf)
    }
}

/// HTTP + cache front-end for the distribution index.
///
/// Generic over the [`HttpFetch`] backend so tests can inject a deterministic
/// transport; defaults to [`UreqFetch`].
pub struct RegistryClient<H: HttpFetch = UreqFetch> {
    config: RegistryConfig,
    cache: RegistryCache,
    http: H,
}

impl RegistryClient<UreqFetch> {
    /// Construct a client using the real `ureq` transport.
    ///
    /// `cache_root` is the registry cache directory
    /// (`~/.cache/anolisa/registry`); it is created lazily on first write.
    pub fn new(config: RegistryConfig, cache_root: PathBuf) -> Self {
        Self::with_http(config, cache_root, UreqFetch::default())
    }
}

impl<H: HttpFetch> RegistryClient<H> {
    /// Construct a client with a custom HTTP backend (tests, mirrors).
    pub fn with_http(config: RegistryConfig, cache_root: PathBuf, http: H) -> Self {
        Self {
            config,
            cache: RegistryCache::new(cache_root),
            http,
        }
    }

    /// Return a parsed index plus how it was obtained.
    ///
    /// Decision table (design §1.6):
    /// - TTL valid → [`IndexFreshness::CacheHit`] (no network).
    /// - TTL expired + fetch ok → overwrite cache, [`IndexFreshness::Fresh`].
    /// - TTL expired + fetch fails + `offline_fallback` + cache present →
    ///   [`IndexFreshness::StaleOffline`].
    /// - TTL expired + fetch fails + no fallback (or no cache) →
    ///   [`RegistryError::Offline`].
    ///
    /// # Errors
    /// [`RegistryError::Offline`] when a stale index cannot be refreshed and
    /// no fallback applies; [`RegistryError::Parse`] on malformed index TOML;
    /// [`RegistryError::Io`] on cache read/write failure.
    pub fn fetch_index(&self) -> Result<(DistributionIndex, IndexFreshness), RegistryError> {
        if self.cache.is_fresh(self.config.cache_ttl) {
            let index = self.cache.load_index()?;
            return Ok((index, IndexFreshness::CacheHit));
        }

        match self.http.get(&self.config.index_url) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|e| RegistryError::Parse {
                    reason: format!("index body is not valid UTF-8: {e}"),
                })?;
                let index = DistributionIndex::from_toml_str(&text)
                    .map_err(|reason| RegistryError::Parse { reason })?;
                self.cache.store(&text)?;
                Ok((index, IndexFreshness::Fresh))
            }
            Err(failure) => {
                // Could not refresh. Serve a stale cache iff allowed and present.
                if self.config.offline_fallback && self.cache.has_index() {
                    let index = self.cache.load_index()?;
                    Ok((index, IndexFreshness::StaleOffline))
                } else {
                    Err(RegistryError::Offline {
                        url: self.config.index_url.clone(),
                        reason: failure.to_string(),
                    })
                }
            }
        }
    }

    /// Fetch and parse a component version's metadata — a byte-for-byte copy of
    /// the artifact's `.anolisa/component.toml` (publish contract §3).
    ///
    /// The meta URL is `artifact_url` with its final path segment replaced by
    /// `meta.toml` (same-directory convention, frozen in contract §3). Results
    /// are cached forever under `artifacts/<component>-<version>-meta.toml`,
    /// since a published `(component, version)` is immutable.
    ///
    /// Returns `Ok(None)` when the registry has no `meta.toml` for this version
    /// (HTTP 404) so a dry-run can degrade to a no-metadata preview instead of
    /// failing.
    ///
    /// # Errors
    /// [`RegistryError::Offline`] if the endpoint is unreachable (non-404),
    /// [`RegistryError::Parse`] on malformed metadata, [`RegistryError::Io`] on
    /// cache failure.
    pub fn fetch_meta(
        &self,
        component: &str,
        version: &str,
        artifact_url: &str,
    ) -> Result<Option<FetchedMeta>, RegistryError> {
        if let Some(text) = self.cache.read_meta(component, version)? {
            return Ok(Some(FetchedMeta::parse(&text)?));
        }

        let meta_url = derive_meta_url(artifact_url)?;
        match self.http.get(&meta_url) {
            Ok(bytes) => {
                let text = String::from_utf8(bytes).map_err(|e| RegistryError::Parse {
                    reason: format!("meta.toml is not valid UTF-8: {e}"),
                })?;
                // Validate before caching so a corrupt body is never persisted.
                let meta = FetchedMeta::parse(&text)?;
                self.cache.write_meta(component, version, &text)?;
                Ok(Some(meta))
            }
            // No meta published for this version → caller degrades gracefully.
            Err(FetchFailure::Status { code: 404 }) => Ok(None),
            Err(failure) => Err(RegistryError::Offline {
                url: meta_url,
                reason: failure.to_string(),
            }),
        }
    }
}

/// Parsed per-version metadata plus the digest of its raw bytes.
#[derive(Debug, Clone)]
pub struct FetchedMeta {
    /// The metadata parsed with the standard [`ComponentManifest`] parser.
    pub manifest: ComponentManifest,
    /// Lowercase-hex sha256 of the raw `meta.toml` bytes. Contract I3 makes
    /// the artifact's `.anolisa/component.toml` byte-identical to meta.toml,
    /// so this digest is the expected value for the execute-time
    /// plan-vs-artifact consistency check (T1.4).
    pub sha256: String,
}

impl FetchedMeta {
    fn parse(text: &str) -> Result<Self, RegistryError> {
        let manifest =
            ComponentManifest::from_toml_str(text).map_err(|e| RegistryError::Parse {
                reason: e.to_string(),
            })?;
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        let sha256 = format!("{:x}", hasher.finalize());
        Ok(Self { manifest, sha256 })
    }
}

/// Derive the `meta.toml` URL from an artifact URL by replacing the final path
/// segment. E.g. `…/0.5.0/tokenless-0.5.0-linux-x86_64.tar.gz` → `…/0.5.0/meta.toml`.
fn derive_meta_url(artifact_url: &str) -> Result<String, RegistryError> {
    match artifact_url.rfind('/') {
        Some(idx) => Ok(format!("{}/meta.toml", &artifact_url[..idx])),
        None => Err(RegistryError::Parse {
            reason: format!("artifact url has no path segment: {artifact_url}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use tempfile::TempDir;

    const SAMPLE_INDEX: &str = r#"
        schema_version = 1
        channel = "stable"
        [[entries]]
        component = "tokenless"
        version = "0.5.0"
        channel = "stable"
        artifact_type = "tar_gz"
        backend = "local-file"
        url = "http://127.0.0.1:8080/x.tar.gz"
        os = "linux"
        arch = "x86_64"
    "#;

    /// Canned outcome for [`FakeHttp`], rebuilt fresh on each `get` since
    /// [`FetchFailure`] is not `Clone`.
    enum Outcome {
        Body(Vec<u8>),
        Network,
        Status(u16),
    }

    /// Records call count; returns a canned body or failure regardless of URL.
    struct FakeHttp {
        calls: Cell<usize>,
        outcome: Outcome,
    }

    impl FakeHttp {
        fn ok(body: &str) -> Self {
            Self {
                calls: Cell::new(0),
                outcome: Outcome::Body(body.as_bytes().to_vec()),
            }
        }
        fn down() -> Self {
            Self {
                calls: Cell::new(0),
                outcome: Outcome::Network,
            }
        }
        fn status(code: u16) -> Self {
            Self {
                calls: Cell::new(0),
                outcome: Outcome::Status(code),
            }
        }
        fn calls(&self) -> usize {
            self.calls.get()
        }
    }

    impl HttpFetch for FakeHttp {
        fn get(&self, _url: &str) -> Result<Vec<u8>, FetchFailure> {
            self.calls.set(self.calls.get() + 1);
            match &self.outcome {
                Outcome::Body(b) => Ok(b.clone()),
                Outcome::Network => Err(FetchFailure::Network {
                    reason: "connection refused".into(),
                }),
                Outcome::Status(code) => Err(FetchFailure::Status { code: *code }),
            }
        }
    }

    /// Minimal-schema `meta.toml` (the tokenless component contract). The V2
    /// parser tolerates the namespaced sections; T2.1 makes them meaningful.
    const META_BODY: &str = r#"
        [component]
        name = "tokenless"
        version = "0.5.0"
        display_name = "Tokenless"

        [component.contract]
        schema_version = "1.0"

        [[component.layout.files]]
        source = "bin/tokenless"
        target = "{bindir}/tokenless"
        type = "executable"

        [component.health_check]
        type = "binary_version"
        binary = "{bindir}/tokenless"
    "#;

    /// (component, version, artifact_url) tuple matching SAMPLE_INDEX's entry.
    const META_ARGS: (&str, &str, &str) = (
        "tokenless",
        "0.5.0",
        "http://127.0.0.1:8080/v1/components/tokenless/0.5.0/x.tar.gz",
    );

    fn cfg(ttl_secs: u64, offline_fallback: bool) -> RegistryConfig {
        RegistryConfig {
            index_url: "http://registry.test/v1/index.toml".into(),
            cache_ttl: Duration::from_secs(ttl_secs),
            offline_fallback,
        }
    }

    #[test]
    fn first_fetch_is_fresh_and_populates_cache() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::ok(SAMPLE_INDEX));
        let (idx, freshness) = client.fetch_index().expect("fetch");
        assert_eq!(freshness, IndexFreshness::Fresh);
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(client.http.calls(), 1);
        // Cache populated for next call.
        assert!(client.cache.has_index());
    }

    #[test]
    fn within_ttl_is_cache_hit_with_zero_network() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        // Prime the cache via a first Fresh fetch.
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::ok(SAMPLE_INDEX));
        client.fetch_index().expect("prime");
        // Second call: still within TTL → no new network request.
        let (_idx, freshness) = client.fetch_index().expect("fetch");
        assert_eq!(freshness, IndexFreshness::CacheHit);
        assert_eq!(client.http.calls(), 1, "no second network request");
    }

    #[test]
    fn expired_ttl_refetches_fresh() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        // ttl = 0 → always stale → always refetch.
        let client = RegistryClient::with_http(cfg(0, true), root, FakeHttp::ok(SAMPLE_INDEX));
        client.fetch_index().expect("prime");
        let (_idx, freshness) = client.fetch_index().expect("fetch");
        assert_eq!(freshness, IndexFreshness::Fresh);
        assert_eq!(client.http.calls(), 2, "expired TTL refetches");
    }

    #[test]
    fn expired_plus_offline_with_cache_serves_stale() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        // Prime cache with a working transport, then go offline.
        RegistryClient::with_http(cfg(0, true), root.clone(), FakeHttp::ok(SAMPLE_INDEX))
            .fetch_index()
            .expect("prime");
        let client = RegistryClient::with_http(cfg(0, true), root, FakeHttp::down());
        let (idx, freshness) = client.fetch_index().expect("stale served");
        assert_eq!(freshness, IndexFreshness::StaleOffline);
        assert_eq!(idx.entries.len(), 1);
    }

    #[test]
    fn expired_plus_offline_without_fallback_errors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        RegistryClient::with_http(cfg(0, true), root.clone(), FakeHttp::ok(SAMPLE_INDEX))
            .fetch_index()
            .expect("prime");
        // offline_fallback = false → Offline error despite cache present.
        let client = RegistryClient::with_http(cfg(0, false), root, FakeHttp::down());
        let err = client.fetch_index().expect_err("must error");
        assert!(matches!(err, RegistryError::Offline { .. }));
    }

    #[test]
    fn first_fetch_offline_no_cache_errors() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::down());
        let err = client.fetch_index().expect_err("no cache, net down");
        assert!(matches!(err, RegistryError::Offline { .. }));
    }

    #[test]
    fn derive_meta_url_replaces_last_segment() {
        let url = "http://h/v1/components/tokenless/0.5.0/tokenless-0.5.0-linux-x86_64.tar.gz";
        assert_eq!(
            derive_meta_url(url).unwrap(),
            "http://h/v1/components/tokenless/0.5.0/meta.toml"
        );
    }

    #[test]
    fn fetch_meta_returns_manifest_digest_and_caches() {
        let (comp, ver, url) = META_ARGS;
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::ok(META_BODY));
        let meta = client
            .fetch_meta(comp, ver, url)
            .expect("fetch")
            .expect("meta present");
        assert_eq!(meta.manifest.component.name, "tokenless");
        // Digest must be the sha256 of the raw bytes the fake served.
        let mut hasher = Sha256::new();
        hasher.update(META_BODY.as_bytes());
        assert_eq!(meta.sha256, format!("{:x}", hasher.finalize()));
        assert_eq!(client.http.calls(), 1);
    }

    #[test]
    fn fetch_meta_second_call_hits_cache_with_same_digest() {
        let (comp, ver, url) = META_ARGS;
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::ok(META_BODY));
        let first = client
            .fetch_meta(comp, ver, url)
            .expect("prime")
            .expect("present");
        let second = client
            .fetch_meta(comp, ver, url)
            .expect("cached")
            .expect("present");
        assert_eq!(client.http.calls(), 1, "version meta is immutable, cached");
        assert_eq!(first.sha256, second.sha256, "cache must preserve bytes");
    }

    #[test]
    fn fetch_meta_404_returns_none() {
        let (comp, ver, url) = META_ARGS;
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::status(404));
        let got = client.fetch_meta(comp, ver, url).expect("graceful");
        assert!(got.is_none(), "missing meta degrades to None, not error");
    }

    #[test]
    fn fetch_meta_network_down_errors() {
        let (comp, ver, url) = META_ARGS;
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("registry");
        let client = RegistryClient::with_http(cfg(3600, true), root, FakeHttp::down());
        let err = client.fetch_meta(comp, ver, url).expect_err("net down");
        assert!(matches!(err, RegistryError::Offline { .. }));
    }
}
