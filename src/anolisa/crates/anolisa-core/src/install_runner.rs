//! Install runner: copy a cached artifact into the ANOLISA-owned layout.
//!
//! This milestone only supports two backends:
//! * `binary` - the cached file IS the installed binary (one file in,
//!   one file out). Manifest must declare exactly one dest.
//! * `tar_gz` - extract a gzipped tar archive, then copy each entry
//!   whose basename matches a manifest dest into that dest.
//!
//! All destinations must resolve under one of the ANOLISA-owned roots
//! (`bin_dir`, `etc_dir`, `state_dir`, `lib_dir`, `libexec_dir`, `datadir`,
//! `log_dir`, `cache_dir`). Anything else is rejected as
//! `InstallError::ExternalPath`. The runner refuses to modify or even
//! create files outside those roots, so a failed install can roll back by
//! deleting just the paths it returns.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anolisa_platform::fs_layout::FsLayout;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::manifest::FileKind;

/// Wire-form `artifact_type` strings the install runner understands today.
///
/// Single source of truth shared with `contract_lint` so a new entry in
/// `DistributionIndex` cannot pass lint and then fail at runtime —
/// `lint_distribution` rejects any `artifact_type` not in this list with
/// `E_UNSUPPORTED_ARTIFACT_TYPE`, so unimplemented backends never enter a
/// `Ready` plan. Keep these in sync with the `match` arm in
/// [`InstallRunner::install_files`]; if you add `rpm`/`deb`/`oci`, push the
/// label here and the lint will start accepting it.
pub const SUPPORTED_ARTIFACT_TYPES: &[&str] = &["binary", "tar_gz"];

/// One destination file written by the runner, with the sha256 of the
/// installed bytes. Sub-C records these in `InstalledState`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFile {
    /// Absolute destination path actually written.
    pub path: PathBuf,
    /// Lowercase-hex sha256 of the installed bytes.
    pub sha256: String,
}

/// Source-to-destination mapping after manifest layout substitution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedInstallFile {
    /// Optional archive entry path. `None` means match by destination
    /// basename for backward-compatible manifests. For
    /// [`FileKind::Symlink`] entries this is the link's referent — an
    /// absolute layout-expanded path, not an archive member.
    pub source: Option<String>,
    /// Absolute destination after layout-template substitution.
    pub dest: PathBuf,
    /// Optional Unix file mode from the component manifest, e.g. `"0644"`.
    pub mode: Option<String>,
    /// File role; [`FileKind::Symlink`] entries are created after the
    /// regular files instead of being extracted from the artifact.
    pub kind: FileKind,
}

impl ResolvedInstallFile {
    /// Build a destination-only mapping used by legacy callers that do
    /// not distinguish archive source paths.
    pub fn dest_only(dest: PathBuf) -> Self {
        Self {
            source: None,
            dest,
            mode: None,
            kind: FileKind::Data,
        }
    }
}

/// Aggregate result of a single [`InstallRunner::install`] call.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// One entry per destination written, in `resolved_dests` order.
    pub files: Vec<InstalledFile>,
}

/// Failure modes for [`InstallRunner::install`].
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// Artifact backend is not implemented by this milestone's runner.
    #[error("artifact_type '{0}' is not supported by this milestone (only 'binary' and 'tar_gz')")]
    UnsupportedArtifactType(String),

    /// Manifest resolved to no destination files.
    #[error("manifest must declare at least one destination file")]
    NoDestinations,

    /// Symlink layout entry lacks a `source` (the link referent).
    #[error("symlink destination '{path}' declares no source (link referent)")]
    SymlinkMissingSource {
        /// Symlink destination with no referent to point at.
        path: PathBuf,
    },

    /// Raw binary artifacts can only map to one installed destination.
    #[error("'binary' install requires exactly one manifest dest, got {0}")]
    BinaryRequiresSingleDest(usize),

    /// Destination is outside the active ANOLISA-owned layout.
    #[error("destination '{path}' is not under an ANOLISA-owned root")]
    ExternalPath {
        /// Rejected destination path.
        path: PathBuf,
    },

    /// Destination contains traversal syntax after template rendering.
    #[error(
        "destination '{path}' contains a '.' or '..' segment — refuse to install via traversal"
    )]
    TraversalSegment {
        /// Rejected destination path.
        path: PathBuf,
    },

    /// Manifest requested a file mode that is not valid octal notation.
    #[error("destination '{path}' has invalid install mode '{mode}'")]
    InvalidMode {
        /// Destination whose mode could not be parsed.
        path: PathBuf,
        /// Raw manifest mode string.
        mode: String,
    },

    /// Fresh-install milestone refuses to overwrite existing files.
    #[error("destination '{path}' already exists — refuses to overwrite")]
    DestExists {
        /// Existing destination path.
        path: PathBuf,
    },

    /// Two manifest/archive entries resolved to the same destination.
    #[error("destination '{path}' is declared more than once")]
    DuplicateDestination {
        /// Duplicate destination path.
        path: PathBuf,
    },

    /// Layout substitution failed to consume a template placeholder.
    #[error(
        "destination '{path}' resolved to an unrendered template — manifest variable not substituted"
    )]
    UnresolvedTemplate {
        /// Destination still containing template syntax.
        path: PathBuf,
    },

    /// Archive did not contain the requested source entry.
    #[error("tar_gz archive entry for dest basename '{basename}' not found")]
    MissingArchiveEntry {
        /// Normalized archive key or legacy destination basename.
        basename: String,
    },

    /// Filesystem access failed while reading the cache or writing a
    /// destination.
    #[error("io error while accessing {path}: {source}")]
    Io {
        /// Path involved in the failed filesystem operation.
        path: PathBuf,
        /// Original I/O error from the OS.
        #[source]
        source: std::io::Error,
    },

    /// Archive stream could not be decoded or read.
    #[error("archive read error: {0}")]
    Archive(String),

    /// Embedded `.anolisa/component.toml` is not valid UTF-8 or could not be
    /// parsed as a component manifest.
    #[error("embedded component manifest could not be parsed: {0}")]
    EmbeddedManifestParse(String),
}

/// Extract and parse the published install contract embedded in a tar.gz
/// artifact at `.anolisa/component.toml`.
///
/// Returns `Ok(None)` when the archive has no such entry. Entry paths are
/// compared after stripping any leading `./` (tar created with `-C dir .`
/// prefixes every path that way).
///
/// This manifest is byte-identical to the registry `meta.toml` (contract
/// I3). Adapter install reads it so the `source`/`dest`/`version` it acts on
/// come from the *published* artifact rather than the dev-tree catalog,
/// which may carry stale build-path sources and lagging versions.
///
/// # Errors
/// [`InstallError::Io`] when the archive cannot be opened or read;
/// [`InstallError::Archive`] when gzip/tar decoding fails;
/// [`InstallError::EmbeddedManifestParse`] when the entry is not valid
/// component-manifest TOML.
pub fn read_embedded_component_manifest(
    artifact: &Path,
) -> Result<Option<crate::manifest::ComponentManifest>, InstallError> {
    let Some(text) = read_embedded_component_manifest_text(artifact)? else {
        return Ok(None);
    };
    let manifest = crate::manifest::ComponentManifest::from_toml_str(&text)
        .map_err(|e| InstallError::EmbeddedManifestParse(e.to_string()))?;
    Ok(Some(manifest))
}

/// Extract the embedded `.anolisa/component.toml` text from a tar.gz
/// artifact.
///
/// Returns `Ok(None)` when the archive has no such entry. This is used when
/// callers need to persist the published component contract byte-for-byte as
/// local install metadata.
///
/// # Errors
/// [`InstallError::Io`] when the archive cannot be opened or read;
/// [`InstallError::Archive`] when gzip/tar decoding fails;
/// [`InstallError::EmbeddedManifestParse`] when the entry is not valid UTF-8.
pub fn read_embedded_component_manifest_text(
    artifact: &Path,
) -> Result<Option<String>, InstallError> {
    let io_err = |source: std::io::Error| InstallError::Io {
        path: artifact.to_path_buf(),
        source,
    };
    let archive_err = |e: std::io::Error| InstallError::Archive(e.to_string());

    let file = File::open(artifact).map_err(io_err)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);
    for entry in archive.entries().map_err(archive_err)? {
        let mut entry = entry.map_err(archive_err)?;
        // Scope the path borrow so `read_to_end` can take `&mut entry`.
        let is_manifest = {
            let path = entry.path().map_err(archive_err)?;
            let normalized = path.strip_prefix("./").unwrap_or(&path);
            normalized == Path::new(".anolisa/component.toml")
        };
        if is_manifest {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(io_err)?;
            let text = String::from_utf8(bytes)
                .map_err(|e| InstallError::EmbeddedManifestParse(e.to_string()))?;
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// Stateless installer bound to an [`FsLayout`] for ANOLISA-owned-root
/// validation. Construct one per `enable` invocation.
pub struct InstallRunner<'a> {
    layout: &'a FsLayout,
}

impl<'a> InstallRunner<'a> {
    /// Build a runner over `layout` — used only to validate that every
    /// destination resolves under an ANOLISA-owned root.
    pub fn new(layout: &'a FsLayout) -> Self {
        Self { layout }
    }

    /// Install `cached_artifact` to the destinations in `resolved_dests`,
    /// which must be absolute paths already substituted against the layout
    /// (Sub-C will pass the planner's `ComponentPlan.resolved_files`).
    ///
    /// `artifact_type` is the wire string from the install plan (e.g. "binary",
    /// "tar_gz").
    ///
    /// On success returns one `InstalledFile` per written path with the
    /// final sha256 — Sub-C will copy these into `InstalledState.objects[].files`.
    pub fn install(
        &self,
        artifact_type: &str,
        cached_artifact: &Path,
        resolved_dests: &[PathBuf],
    ) -> Result<InstallOutcome, InstallError> {
        let files: Vec<ResolvedInstallFile> = resolved_dests
            .iter()
            .cloned()
            .map(ResolvedInstallFile::dest_only)
            .collect();
        self.install_files(artifact_type, cached_artifact, &files)
    }

    /// Install files using explicit source-to-destination mappings.
    ///
    /// Source paths are meaningful for archives; raw binaries must still
    /// resolve to exactly one destination. All destinations are validated
    /// before any file is written so a rejected path cannot leave a
    /// partial install behind.
    ///
    /// # Errors
    ///
    /// Fails when the artifact type is unsupported, any destination is
    /// unsafe or already exists, the cache cannot be read, or an archive
    /// lacks a requested entry. If a later write step fails after earlier
    /// paths were created, the runner best-effort removes the paths it
    /// created before returning the original error.
    pub fn install_files(
        &self,
        artifact_type: &str,
        cached_artifact: &Path,
        files: &[ResolvedInstallFile],
    ) -> Result<InstallOutcome, InstallError> {
        if files.is_empty() {
            return Err(InstallError::NoDestinations);
        }
        // Symlink entries never touch the artifact: split them out, install
        // the regular files, then create the links — referents that point at
        // freshly installed files exist by the time the link is made.
        let (links, regular): (Vec<_>, Vec<_>) = files
            .iter()
            .cloned()
            .partition(|f| f.kind == FileKind::Symlink);
        self.validate_symlink_entries(&links)?;
        if regular.is_empty() {
            // A links-only manifest has no use for the downloaded artifact —
            // treat it as the same defect as declaring no files at all.
            return Err(InstallError::NoDestinations);
        }
        let mut outcome = match artifact_type {
            "binary" => {
                self.validate_install_targets(&regular)?;
                self.install_binary(cached_artifact, &regular)
            }
            "tar_gz" => self.install_tar_gz(cached_artifact, &regular),
            other => Err(InstallError::UnsupportedArtifactType(other.to_string())),
        }?;
        for link in &links {
            match create_symlink(link) {
                Ok(installed) => outcome.files.push(installed),
                Err(err) => {
                    rollback_installed_files(&outcome.files);
                    return Err(err);
                }
            }
        }
        Ok(outcome)
    }

    /// Up-front checks for symlink entries, run before any byte lands so a
    /// rejected link cannot leave a half-finished install: referent
    /// declared and ANOLISA-owned, destination ANOLISA-owned and vacant.
    fn validate_symlink_entries(&self, links: &[ResolvedInstallFile]) -> Result<(), InstallError> {
        let mut seen = BTreeSet::new();
        for link in links {
            let referent =
                link.source
                    .as_deref()
                    .ok_or_else(|| InstallError::SymlinkMissingSource {
                        path: link.dest.clone(),
                    })?;
            // A link must not point outside the owned roots any more than a
            // regular file may be written there.
            self.validate_dest(Path::new(referent))?;
            self.validate_dest(&link.dest)?;
            if !seen.insert(link.dest.clone()) {
                return Err(InstallError::DuplicateDestination {
                    path: link.dest.clone(),
                });
            }
            // Same fresh-install rule as regular destinations, with
            // symlink_metadata so an existing broken link is still refused.
            match fs::symlink_metadata(&link.dest) {
                Ok(_) => {
                    return Err(InstallError::DestExists {
                        path: link.dest.clone(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(InstallError::Io {
                        path: link.dest.clone(),
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    fn install_binary(
        &self,
        cached_artifact: &Path,
        files: &[ResolvedInstallFile],
    ) -> Result<InstallOutcome, InstallError> {
        if files.len() != 1 {
            return Err(InstallError::BinaryRequiresSingleDest(files.len()));
        }
        let dest = &files[0].dest;
        let bytes = read_file_bytes(cached_artifact)?;
        let installed = write_dest_atomic(dest, &bytes, files[0].mode.as_deref())?;
        Ok(InstallOutcome {
            files: vec![installed],
        })
    }

    fn install_tar_gz(
        &self,
        cached_artifact: &Path,
        files: &[ResolvedInstallFile],
    ) -> Result<InstallOutcome, InstallError> {
        let entries = read_tar_gz_entries(cached_artifact)?;

        let mut expanded: Vec<(ResolvedInstallFile, Vec<u8>)> = Vec::new();
        for file in files {
            if let Some(source) = file.source.as_deref()
                && archive_source_is_dir(source)
            {
                let prefix = normalize_archive_key(source);
                let prefix = prefix.trim_end_matches('/');
                let mut matched = false;
                for (key, bytes) in &entries.full_paths {
                    let Some(relative) = archive_relative_under(key, prefix) else {
                        continue;
                    };
                    if relative.is_empty() {
                        continue;
                    }
                    matched = true;
                    expanded.push((
                        ResolvedInstallFile {
                            source: Some(key.clone()),
                            dest: file.dest.join(relative),
                            mode: file.mode.clone(),
                            kind: file.kind,
                        },
                        bytes.clone(),
                    ));
                }
                if !matched {
                    return Err(InstallError::MissingArchiveEntry {
                        basename: format!("{prefix}/"),
                    });
                }
                continue;
            }

            let key = archive_source_key(file)?;
            let bytes =
                entries
                    .lookup
                    .get(&key)
                    .ok_or_else(|| InstallError::MissingArchiveEntry {
                        basename: key.clone(),
                    })?;
            expanded.push((file.clone(), bytes.clone()));
        }

        let expanded_files: Vec<ResolvedInstallFile> =
            expanded.iter().map(|(file, _)| file.clone()).collect();
        self.validate_install_targets(&expanded_files)?;

        let mut out = Vec::with_capacity(expanded.len());
        for (file, bytes) in expanded {
            match write_dest_atomic(&file.dest, &bytes, file.mode.as_deref()) {
                Ok(installed) => out.push(installed),
                Err(err) => {
                    rollback_installed_files(&out);
                    return Err(err);
                }
            }
        }
        Ok(InstallOutcome { files: out })
    }

    fn validate_dest(&self, dest: &Path) -> Result<(), InstallError> {
        if dest.to_string_lossy().contains('{') {
            return Err(InstallError::UnresolvedTemplate {
                path: dest.to_path_buf(),
            });
        }
        // Shared lexical + canonical boundary check (see path_safety).
        // Uninstall uses the same helper before backup/remove so the two
        // verbs cannot drift out of lockstep on what counts as
        // "ANOLISA-owned".
        crate::path_safety::validate_owned_path(self.layout, dest).map_err(|err| match err {
            crate::path_safety::PathBoundaryError::Traversal { path } => {
                InstallError::TraversalSegment { path }
            }
            crate::path_safety::PathBoundaryError::External { path } => {
                InstallError::ExternalPath { path }
            }
        })
    }

    fn validate_install_targets(&self, files: &[ResolvedInstallFile]) -> Result<(), InstallError> {
        let mut seen = BTreeSet::new();
        for file in files {
            if !seen.insert(file.dest.clone()) {
                return Err(InstallError::DuplicateDestination {
                    path: file.dest.clone(),
                });
            }
            self.validate_dest(&file.dest)?;
        }
        // Fresh-install only for P1-F: refuse to overwrite anything already
        // on disk. Backup/restore of pre-existing ANOLISA-owned files lands
        // in P1-G; until then, the runner must never silently clobber.
        // Check all dests up front so a partial run can't leave half-written
        // siblings behind. Use `symlink_metadata` rather than `exists()` so
        // a broken symlink (target missing, `exists()` returns false) is
        // still caught and refused.
        for file in files {
            match fs::symlink_metadata(&file.dest) {
                Ok(_) => {
                    return Err(InstallError::DestExists {
                        path: file.dest.clone(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(InstallError::Io {
                        path: file.dest.clone(),
                        source,
                    });
                }
            }
        }
        Ok(())
    }
}

fn read_file_bytes(path: &Path) -> Result<Vec<u8>, InstallError> {
    fs::read(path).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Tar entries keyed by their full archive path plus the legacy lookup map.
struct TarGzEntries {
    full_paths: BTreeMap<String, Vec<u8>>,
    lookup: BTreeMap<String, Vec<u8>>,
}

/// Last-write-wins on duplicate archive keys. Entries are addressable both by
/// full archive path (for manifest `source`) and basename (legacy behavior).
fn read_tar_gz_entries(path: &Path) -> Result<TarGzEntries, InstallError> {
    let file = File::open(path).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = Archive::new(GzDecoder::new(file));
    let mut full_paths: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut lookup: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|e| InstallError::Archive(format!("entries: {e}")))?;
    for entry_res in entries {
        let mut entry = entry_res.map_err(|e| InstallError::Archive(format!("entry: {e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let entry_path = entry
            .path()
            .map_err(|e| InstallError::Archive(format!("path: {e}")))?
            .into_owned();
        let Some(path_key) = archive_key_from_path(&entry_path)? else {
            continue;
        };
        let basename = path_key.rsplit('/').next().map(str::to_string);
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| InstallError::Archive(format!("read entry '{path_key}': {e}")))?;
        if let Some(basename) = basename {
            lookup.insert(basename, buf.clone());
        }
        lookup.insert(path_key.clone(), buf.clone());
        full_paths.insert(path_key, buf);
    }
    Ok(TarGzEntries { full_paths, lookup })
}

fn archive_source_key(file: &ResolvedInstallFile) -> Result<String, InstallError> {
    let key = match file.source.as_deref() {
        Some(source) => normalize_archive_key(source),
        None => file
            .dest
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
    };
    if key.is_empty() {
        return Err(InstallError::ExternalPath {
            path: file.dest.clone(),
        });
    }
    Ok(key)
}

fn archive_source_is_dir(source: &str) -> bool {
    source.ends_with('/')
}

fn archive_relative_under<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(key);
    }
    let rest = key.strip_prefix(prefix)?;
    rest.strip_prefix('/')
}

fn normalize_archive_key(path: &str) -> String {
    path.trim_start_matches("./").to_string()
}

fn archive_key_from_path(path: &Path) -> Result<Option<String>, InstallError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Ok(None);
                };
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(InstallError::Archive(format!(
                    "unsafe archive entry path '{}'",
                    path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("/")))
    }
}

/// Create one validated symlink entry and hash what it resolves to.
///
/// The recorded sha256 is the *referent's* content (a `fs::read` of the
/// link follows it), so integrity checks that re-hash owned paths see the
/// same digest whether they visit the link or the file it points at. A
/// referent that does not exist fails here: installing a dangling
/// convenience link would be a manifest defect, not a usable install.
fn create_symlink(link: &ResolvedInstallFile) -> Result<InstalledFile, InstallError> {
    // Validated in validate_symlink_entries; unreachable here.
    let referent = link
        .source
        .as_deref()
        .ok_or_else(|| InstallError::SymlinkMissingSource {
            path: link.dest.clone(),
        })?;
    if let Some(parent) = link.dest.parent() {
        fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::os::unix::fs::symlink(referent, &link.dest).map_err(|source| InstallError::Io {
        path: link.dest.clone(),
        source,
    })?;
    let bytes = match fs::read(&link.dest) {
        Ok(b) => b,
        Err(source) => {
            // Don't leave a dangling link behind the error.
            let _ = fs::remove_file(&link.dest);
            return Err(InstallError::Io {
                path: PathBuf::from(referent),
                source,
            });
        }
    };
    Ok(InstalledFile {
        path: link.dest.clone(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

fn rollback_installed_files(files: &[InstalledFile]) {
    for file in files.iter().rev() {
        let _ = fs::remove_file(&file.path);
    }
}

fn write_dest_atomic(
    dest: &Path,
    bytes: &[u8],
    mode: Option<&str>,
) -> Result<InstalledFile, InstallError> {
    #[cfg(unix)]
    let parsed_mode = parse_unix_mode(mode, dest)?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let tmp = tmp_sibling(dest);
    let sha = match stream_write_and_hash(&tmp, bytes) {
        Ok(h) => h,
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(parsed_mode);
        if let Err(source) = fs::set_permissions(&tmp, perms) {
            let _ = fs::remove_file(&tmp);
            return Err(InstallError::Io {
                path: tmp.clone(),
                source,
            });
        }
    }
    fs::rename(&tmp, dest).map_err(|source| {
        let _ = fs::remove_file(&tmp);
        InstallError::Io {
            path: dest.to_path_buf(),
            source,
        }
    })?;
    Ok(InstalledFile {
        path: dest.to_path_buf(),
        sha256: sha,
    })
}

#[cfg(unix)]
fn parse_unix_mode(mode: Option<&str>, dest: &Path) -> Result<u32, InstallError> {
    const DEFAULT_MODE: u32 = 0o755;
    let Some(raw) = mode else {
        return Ok(DEFAULT_MODE);
    };
    let trimmed = raw.trim();
    let octal = trimmed.strip_prefix("0o").unwrap_or(trimmed);
    let parsed = u32::from_str_radix(octal, 8).map_err(|_| InstallError::InvalidMode {
        path: dest.to_path_buf(),
        mode: raw.to_string(),
    })?;
    if parsed > 0o7777 {
        return Err(InstallError::InvalidMode {
            path: dest.to_path_buf(),
            mode: raw.to_string(),
        });
    }
    Ok(parsed)
}

fn stream_write_and_hash(tmp: &Path, bytes: &[u8]) -> Result<String, InstallError> {
    // Security-critical: open the tmp sibling with O_CREAT|O_EXCL so a
    // pre-placed symlink (or any other existing entry) fails the open
    // with EEXIST/ELOOP instead of letting us write through it to a
    // path outside the ANOLISA-owned roots. On Unix we additionally pass
    // O_NOFOLLOW as belt-and-suspenders: even on a kernel that resolves
    // O_CREAT|O_EXCL race-y vs a concurrently-planted symlink, the final
    // component cannot be followed. `File::create` (the old code) did
    // NOT do either — it opened with O_TRUNC and followed symlinks,
    // which is exactly the hole this hardens against.
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut out = opts.open(tmp).map_err(|source| InstallError::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(8 * 1024) {
        hasher.update(chunk);
        out.write_all(chunk).map_err(|source| InstallError::Io {
            path: tmp.to_path_buf(),
            source,
        })?;
    }
    out.flush().map_err(|source| InstallError::Io {
        path: tmp.to_path_buf(),
        source,
    })?;
    Ok(to_lower_hex(&hasher.finalize()))
}

fn tmp_sibling(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

fn to_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};
    use tempfile::tempdir;

    fn layout_for(home: &Path) -> FsLayout {
        FsLayout::user_with_overrides(home.to_path_buf(), None, None, None, None, None)
    }

    fn write_cached(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        to_lower_hex(&h.finalize())
    }

    fn build_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf: Vec<u8> = Vec::new();
        let enc = GzEncoder::new(buf, Compression::default());
        let mut tar = Builder::new(enc);
        for (path, data) in entries {
            let mut hdr = Header::new_gnu();
            hdr.set_size(data.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar.append_data(&mut hdr, path, *data).unwrap();
        }
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn binary_install_single_dest_succeeds() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let payload = b"fake-binary-bytes";
        let cached = write_cached(cache.path(), "agentsight", payload);
        let dest = layout.bin_dir.join("agentsight");

        let outcome = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect("install ok");

        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].path, dest);
        assert_eq!(outcome.files[0].sha256, sha256_of(payload));
        assert!(dest.exists());
        let got = fs::read(&dest).unwrap();
        assert_eq!(got, payload);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }
    }

    #[test]
    fn binary_install_two_dests_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let d1 = layout.bin_dir.join("a");
        let d2 = layout.bin_dir.join("b");
        let err = runner
            .install("binary", &cached, &[d1, d2])
            .expect_err("must error");
        match err {
            InstallError::BinaryRequiresSingleDest(n) => assert_eq!(n, 2),
            other => panic!("expected BinaryRequiresSingleDest, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_unresolved_template_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = PathBuf::from("{bindir}/foo");
        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must error");
        match err {
            InstallError::UnresolvedTemplate { path } => assert_eq!(path, dest),
            other => panic!("expected UnresolvedTemplate, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_external_path_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = PathBuf::from("/tmp/escape/foo");
        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must error");
        match err {
            InstallError::ExternalPath { path } => assert_eq!(path, dest),
            other => panic!("expected ExternalPath, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_creates_missing_parent_dirs() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"deep");

        let dest = layout.state_dir.join("sub").join("deep").join("file.bin");
        let outcome = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect("install ok");
        assert!(dest.exists());
        assert_eq!(outcome.files[0].sha256, sha256_of(b"deep"));
    }

    #[test]
    fn tar_gz_install_extracts_matching_basenames() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let bin_bytes: &[u8] = b"agentsight-binary";
        let data_bytes: &[u8] = b"data-file-contents";
        let gz = build_tar_gz(&[
            ("bin/agentsight", bin_bytes),
            ("share/data.toml", data_bytes),
        ]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest_bin = layout.bin_dir.join("agentsight");
        let dest_data = layout.datadir.join("data.toml");
        let outcome = runner
            .install("tar_gz", &cached, &[dest_bin.clone(), dest_data.clone()])
            .expect("install ok");

        assert_eq!(outcome.files.len(), 2);
        assert_eq!(fs::read(&dest_bin).unwrap(), bin_bytes);
        assert_eq!(fs::read(&dest_data).unwrap(), data_bytes);
        assert_eq!(outcome.files[0].sha256, sha256_of(bin_bytes));
        assert_eq!(outcome.files[1].sha256, sha256_of(data_bytes));
    }

    #[test]
    fn tar_gz_install_uses_source_but_writes_dest() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let payload: &[u8] = b"tool-bytes";
        let gz = build_tar_gz(&[("target/release/source-name", payload)]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = layout.bin_dir.join("dest-name");
        let outcome = runner
            .install_files(
                "tar_gz",
                &cached,
                &[ResolvedInstallFile {
                    source: Some("target/release/source-name".to_string()),
                    dest: dest.clone(),
                    mode: None,
                    kind: FileKind::Data,
                }],
            )
            .expect("install ok");

        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.files[0].path, dest);
        assert_eq!(fs::read(&outcome.files[0].path).unwrap(), payload);
    }

    #[test]
    fn tar_gz_install_expands_directory_source_prefix() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let manifest: &[u8] = br#"{"name":"tokenless"}"#;
        let script: &[u8] = b"console.log('ok');";
        let gz = build_tar_gz(&[
            ("target/release/openclaw-plugin/plugin.json", manifest),
            ("target/release/openclaw-plugin/dist/index.js", script),
            ("target/release/other-plugin/ignored.txt", b"ignored"),
        ]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest_root = layout.datadir.join("adapters/tokenless/openclaw");
        let outcome = runner
            .install_files(
                "tar_gz",
                &cached,
                &[ResolvedInstallFile {
                    source: Some("target/release/openclaw-plugin/".to_string()),
                    dest: dest_root.clone(),
                    mode: Some("0644".to_string()),
                    kind: FileKind::Data,
                }],
            )
            .expect("install ok");

        assert_eq!(outcome.files.len(), 2);
        assert_eq!(fs::read(dest_root.join("plugin.json")).unwrap(), manifest);
        assert_eq!(fs::read(dest_root.join("dist/index.js")).unwrap(), script);
        assert!(!dest_root.join("ignored.txt").exists());
    }

    #[test]
    fn tar_gz_install_rejects_unsafe_archive_paths() {
        let err = archive_key_from_path(Path::new("../escape.txt"))
            .expect_err("must reject unsafe archive path");
        match err {
            InstallError::Archive(msg) => assert!(msg.contains("unsafe archive entry path")),
            other => panic!("expected Archive, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn install_files_honors_manifest_mode() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let payload: &[u8] = b"config-bytes";
        let gz = build_tar_gz(&[("share/config.toml", payload)]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = layout.datadir.join("config.toml");
        runner
            .install_files(
                "tar_gz",
                &cached,
                &[ResolvedInstallFile {
                    source: Some("share/config.toml".to_string()),
                    dest: dest.clone(),
                    mode: Some("0644".to_string()),
                    kind: FileKind::Data,
                }],
            )
            .expect("install ok");

        let mode = fs::metadata(dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }

    #[test]
    fn invalid_mode_rejected_without_tmp_sibling() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "tool", b"tool-bytes");

        let dest = layout.bin_dir.join("tool");
        let err = runner
            .install_files(
                "binary",
                &cached,
                &[ResolvedInstallFile {
                    source: None,
                    dest: dest.clone(),
                    mode: Some("not-octal".to_string()),
                    kind: FileKind::Data,
                }],
            )
            .expect_err("must reject invalid mode");
        match err {
            InstallError::InvalidMode { path, .. } => assert_eq!(path, dest),
            other => panic!("expected InvalidMode, got {other:?}"),
        }
        assert!(!dest.exists(), "destination must not be created");
        assert!(!tmp_sibling(&dest).exists(), "tmp sibling must be cleaned");
    }

    #[test]
    fn tar_gz_install_missing_entry_reports_basename() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/something-else", b"x")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = layout.bin_dir.join("missing");
        let err = runner
            .install("tar_gz", &cached, &[dest])
            .expect_err("must error");
        match err {
            InstallError::MissingArchiveEntry { basename } => assert_eq!(basename, "missing"),
            other => panic!("expected MissingArchiveEntry, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_artifact_type_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = layout.bin_dir.join("a");
        let err = runner
            .install("rpm", &cached, &[dest])
            .expect_err("must error");
        match err {
            InstallError::UnsupportedArtifactType(s) => assert_eq!(s, "rpm"),
            other => panic!("expected UnsupportedArtifactType, got {other:?}"),
        }
    }

    #[test]
    fn no_dests_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let err = runner
            .install("binary", &cached, &[])
            .expect_err("must error");
        assert!(matches!(err, InstallError::NoDestinations));
    }

    #[test]
    fn binary_install_refuses_to_overwrite_existing_dest() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let cached = write_cached(cache.path(), "agentsight", b"v2-bytes");
        let dest = layout.bin_dir.join("agentsight");

        // Pre-existing file from a prior install / external source.
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, b"v1-bytes").unwrap();

        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("second install must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest),
            other => panic!("expected DestExists, got {other:?}"),
        }

        // Pre-existing file must be untouched — and no .tmp sibling left behind.
        assert_eq!(std::fs::read(&dest).unwrap(), b"v1-bytes");
        let tmp = tmp_sibling(&dest);
        assert!(!tmp.exists(), ".tmp sibling must not be created");
    }

    #[test]
    fn tar_gz_install_refuses_when_any_dest_preexists() {
        // Pre-existence check runs before extraction, so neither dest is
        // written even if only one of them collides.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let bin_bytes: &[u8] = b"agentsight-binary";
        let data_bytes: &[u8] = b"data-file-contents";
        let gz = build_tar_gz(&[
            ("bin/agentsight", bin_bytes),
            ("share/data.toml", data_bytes),
        ]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest_bin = layout.bin_dir.join("agentsight");
        let dest_data = layout.datadir.join("data.toml");
        std::fs::create_dir_all(dest_data.parent().unwrap()).unwrap();
        std::fs::write(&dest_data, b"existing-data").unwrap();

        let err = runner
            .install("tar_gz", &cached, &[dest_bin.clone(), dest_data.clone()])
            .expect_err("must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest_data),
            other => panic!("expected DestExists, got {other:?}"),
        }
        assert!(!dest_bin.exists(), "bin dest must not be created");
        assert_eq!(std::fs::read(&dest_data).unwrap(), b"existing-data");
    }

    #[test]
    fn binary_install_dotdot_segment_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        // dest = <bin_dir>/../escape/file — passes the old lexical
        // starts_with check but would write outside bin_dir.
        let dest = layout.bin_dir.join("..").join("escape").join("file");
        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must reject");
        match err {
            InstallError::TraversalSegment { path } => assert_eq!(path, dest),
            other => panic!("expected TraversalSegment, got {other:?}"),
        }
    }

    #[test]
    fn binary_install_dotdot_at_tail_rejected() {
        // `..` as the final segment would resolve to a directory and let
        // rename overwrite something the user did not name. Same defense
        // as the mid-path case but covers the tail position explicitly.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let dest = layout.bin_dir.join("sub").join("..");
        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must reject");
        match err {
            InstallError::TraversalSegment { path } => assert_eq!(path, dest),
            other => panic!("expected TraversalSegment, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_refuses_broken_symlink_dest() {
        // exists() returns false for a broken symlink (target missing) but
        // symlink_metadata() returns Ok. We must treat the broken symlink
        // as "occupied" and refuse, otherwise rename() would silently
        // replace it.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "agentsight", b"new-bytes");

        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", &dest).unwrap();
        assert!(!dest.exists(), "test precondition: broken symlink");
        assert!(
            fs::symlink_metadata(&dest).is_ok(),
            "symlink itself present"
        );

        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must refuse");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, dest),
            other => panic!("expected DestExists, got {other:?}"),
        }
        // Symlink untouched.
        assert!(fs::symlink_metadata(&dest).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_symlink_ancestor_escapes_root_rejected() {
        // bin_dir/escape -> <outside>, dest = bin_dir/escape/file. The
        // lexical starts_with check passes (it's literally under bin_dir),
        // but canonicalize_nearest_existing resolves the symlink and the
        // canonical dest no longer lives under the canonical root.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        fs::create_dir_all(&layout.bin_dir).unwrap();
        let escape_link = layout.bin_dir.join("escape");
        std::os::unix::fs::symlink(outside.path(), &escape_link).unwrap();

        let dest = escape_link.join("file");
        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must reject");
        assert!(
            matches!(err, InstallError::ExternalPath { ref path } if path == &dest),
            "expected ExternalPath for symlink-escape, got {err:?}",
        );
        assert!(
            !outside.path().join("file").exists(),
            "must not write through the symlink",
        );
    }

    #[cfg(unix)]
    #[test]
    fn binary_install_refuses_when_tmp_sibling_is_a_symlink() {
        // The atomic-write step writes to `{dest}.tmp` and then rename(2)s
        // it into place. If `{dest}.tmp` is a pre-placed symlink to a file
        // outside the ANOLISA-owned roots, the old code (`File::create`)
        // would follow it and corrupt that external file — bypassing
        // every dest-side guard we just added. The fix opens with
        // O_CREAT|O_EXCL (+ O_NOFOLLOW on Unix) so the open itself fails.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "agentsight", b"new-bytes");

        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        // The plant lives at `{dest}.tmp` — the exact path
        // `tmp_sibling(dest)` returns — and targets an external file.
        let outside_target = outside.path().join("victim");
        fs::write(&outside_target, b"untouched-bytes").unwrap();
        let tmp_plant = {
            let mut s = dest.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::os::unix::fs::symlink(&outside_target, &tmp_plant).unwrap();

        let err = runner
            .install("binary", &cached, std::slice::from_ref(&dest))
            .expect_err("must refuse to write through symlinked tmp");
        match err {
            InstallError::Io { path, .. } => assert_eq!(path, tmp_plant),
            other => panic!("expected Io on tmp, got {other:?}"),
        }

        // External file is untouched (the most important invariant).
        let victim_bytes = fs::read(&outside_target).expect("external file readable");
        assert_eq!(
            victim_bytes, b"untouched-bytes",
            "the symlink target must not be written through",
        );
        // Destination was never created.
        assert!(!dest.exists(), "dest must not be installed");
    }

    #[cfg(unix)]
    #[test]
    fn tar_gz_install_refuses_when_tmp_sibling_is_a_symlink() {
        // Same defense applies to the tar_gz backend — it routes through
        // the same `write_dest_atomic` helper so a single fix covers both,
        // but we lock that down with an explicit regression test so a
        // future refactor that splits the helpers cannot regress one
        // backend without tripping a test.
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[
            ("bin/first", b"first-bytes"),
            ("bin/agentsight", b"new-bytes"),
        ]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let first_dest = layout.bin_dir.join("first");
        let dest = layout.bin_dir.join("agentsight");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let outside_target = outside.path().join("victim");
        fs::write(&outside_target, b"untouched-bytes").unwrap();
        let tmp_plant = {
            let mut s = dest.as_os_str().to_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::os::unix::fs::symlink(&outside_target, &tmp_plant).unwrap();

        let err = runner
            .install("tar_gz", &cached, &[first_dest.clone(), dest.clone()])
            .expect_err("must refuse to write through symlinked tmp");
        match err {
            InstallError::Io { path, .. } => assert_eq!(path, tmp_plant),
            other => panic!("expected Io on tmp, got {other:?}"),
        }

        let victim_bytes = fs::read(&outside_target).expect("external file readable");
        assert_eq!(victim_bytes, b"untouched-bytes");
        assert!(!dest.exists());
        assert!(
            !first_dest.exists(),
            "earlier tar_gz writes must roll back when a later write fails"
        );
    }

    #[test]
    fn tar_gz_external_dest_rejected_before_extraction() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let dest = PathBuf::from("/tmp/escape/foo");
        let err = runner
            .install("tar_gz", &cached, &[dest])
            .expect_err("must error");
        assert!(matches!(err, InstallError::ExternalPath { .. }));
        let leaked = layout.bin_dir.join("foo");
        assert!(!leaked.exists(), "must not extract before validating dest");
    }

    fn symlink_entry(referent: &Path, dest: PathBuf) -> ResolvedInstallFile {
        ResolvedInstallFile {
            source: Some(referent.to_string_lossy().into_owned()),
            dest,
            mode: None,
            kind: FileKind::Symlink,
        }
    }

    #[test]
    fn symlink_created_after_regular_files_with_referent_hash() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let payload: &[u8] = b"rtk-bytes";
        let gz = build_tar_gz(&[("libexec/anolisa/tokenless/rtk", payload)]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let referent = layout.libexec_dir.join("tokenless").join("rtk");
        let link_dest = layout.bin_dir.join("rtk");
        let files = vec![
            ResolvedInstallFile {
                source: Some("libexec/anolisa/tokenless/rtk".into()),
                dest: referent.clone(),
                mode: Some("0755".into()),
                kind: FileKind::Data,
            },
            symlink_entry(&referent, link_dest.clone()),
        ];

        let outcome = runner
            .install_files("tar_gz", &cached, &files)
            .expect("install ok");

        assert!(fs::symlink_metadata(&link_dest).unwrap().is_symlink());
        assert_eq!(fs::read_link(&link_dest).unwrap(), referent);
        // Both entries are recorded and the link's digest is the referent's
        // content, matching what an integrity re-hash of the path would see.
        let link_file = outcome
            .files
            .iter()
            .find(|f| f.path == link_dest)
            .expect("link recorded in outcome");
        assert_eq!(link_file.sha256, sha256_of(payload));
    }

    #[test]
    fn symlink_without_source_rejected_before_any_write() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let regular_dest = layout.bin_dir.join("foo");
        let link_dest = layout.bin_dir.join("foo-link");
        let files = vec![
            ResolvedInstallFile::dest_only(regular_dest.clone()),
            ResolvedInstallFile {
                source: None,
                dest: link_dest.clone(),
                mode: None,
                kind: FileKind::Symlink,
            },
        ];

        let err = runner
            .install_files("tar_gz", &cached, &files)
            .expect_err("must error");
        match err {
            InstallError::SymlinkMissingSource { path } => assert_eq!(path, link_dest),
            other => panic!("expected SymlinkMissingSource, got {other:?}"),
        }
        assert!(!regular_dest.exists(), "must validate links before writing");
    }

    #[test]
    fn symlink_dest_exists_rejected_even_for_broken_link() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let referent = layout.bin_dir.join("foo");
        let link_dest = layout.bin_dir.join("foo-link");
        fs::create_dir_all(link_dest.parent().unwrap()).unwrap();
        // Pre-existing *broken* link: plain exists() would miss it.
        std::os::unix::fs::symlink(layout.bin_dir.join("missing"), &link_dest).unwrap();

        let files = vec![
            ResolvedInstallFile::dest_only(referent),
            symlink_entry(&layout.bin_dir.join("foo"), link_dest.clone()),
        ];
        let err = runner
            .install_files("tar_gz", &cached, &files)
            .expect_err("must error");
        match err {
            InstallError::DestExists { path } => assert_eq!(path, link_dest),
            other => panic!("expected DestExists, got {other:?}"),
        }
    }

    #[test]
    fn symlink_referent_outside_owned_roots_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        let external = outside.path().join("victim");
        let files = vec![
            ResolvedInstallFile::dest_only(layout.bin_dir.join("foo")),
            symlink_entry(&external, layout.bin_dir.join("foo-link")),
        ];
        let err = runner
            .install_files("tar_gz", &cached, &files)
            .expect_err("must error");
        match err {
            InstallError::ExternalPath { path } => assert_eq!(path, external),
            other => panic!("expected ExternalPath, got {other:?}"),
        }
    }

    #[test]
    fn symlink_dangling_referent_rejected_and_link_removed() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);

        let gz = build_tar_gz(&[("bin/foo", b"foo-bytes")]);
        let cached = cache.path().join("payload.tar.gz");
        fs::write(&cached, &gz).unwrap();

        // Referent is owned but nothing installs it: the link would dangle.
        let referent = layout.libexec_dir.join("tokenless").join("missing");
        let link_dest = layout.bin_dir.join("missing-link");
        let regular_dest = layout.bin_dir.join("foo");
        let files = vec![
            ResolvedInstallFile::dest_only(regular_dest.clone()),
            symlink_entry(&referent, link_dest.clone()),
        ];
        let err = runner
            .install_files("tar_gz", &cached, &files)
            .expect_err("must error");
        match err {
            InstallError::Io { path, .. } => assert_eq!(path, referent),
            other => panic!("expected Io on referent, got {other:?}"),
        }
        assert!(
            fs::symlink_metadata(&link_dest).is_err(),
            "dangling link must not be left behind"
        );
        assert!(
            !regular_dest.exists(),
            "regular files written before the failed link must be rolled back"
        );
    }

    #[test]
    fn links_only_manifest_rejected() {
        let home = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let layout = layout_for(home.path());
        let runner = InstallRunner::new(&layout);
        let cached = write_cached(cache.path(), "x", b"x");

        let files = vec![symlink_entry(
            &layout.bin_dir.join("foo"),
            layout.bin_dir.join("foo-link"),
        )];
        let err = runner
            .install_files("binary", &cached, &files)
            .expect_err("must error");
        assert!(matches!(err, InstallError::NoDestinations));
    }
}
