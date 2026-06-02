//! Environment readiness checker for Tool Ready feature.
//!
//! Loads per-tool dependency declarations from tool-ready-spec.json.
//! Supports both string format ("jq") and object format ({binary, version, package, manager, ...}).
//! Checks binary availability (with version constraints), config files,
//! permissions, and network connectivity. Generates a structured
//! ready checklist and supports auto-fix via config-driven install engine.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

#[cfg(unix)]
fn current_uid() -> u32 {
    // libc::getuid is a FFI call — requires unsafe block per Rust 2024 edition rules.
    unsafe { libc::getuid() }
}

#[cfg(unix)]
fn is_trusted_path(path: &std::path::Path) -> bool {
    // System paths are always trusted
    if path.starts_with("/usr/share")
        || path.starts_with("/usr/libexec")
        || path.starts_with("/usr/lib/anolisa")
        || path.starts_with("/usr/local/share")
    {
        return true;
    }
    // Resolve symlink target before owner/perm checks
    let check_path = if path.is_symlink() {
        match fs::canonicalize(path) {
            Ok(resolved) => {
                // System targets are always trusted
                if resolved.starts_with("/usr/share")
                    || resolved.starts_with("/usr/libexec")
                    || resolved.starts_with("/usr/lib/anolisa")
                    || resolved.starts_with("/usr/local/share")
                {
                    return true;
                }
                resolved
            }
            Err(_) => return false,
        }
    } else {
        path.to_path_buf()
    };
    // Use symlink_metadata to check the target's metadata (not the symlink itself)
    match fs::symlink_metadata(&check_path) {
        Ok(meta) => {
            let file_uid = meta.uid();
            let current_uid = current_uid();
            if file_uid != current_uid && file_uid != 0 {
                return false;
            }
            let mode = meta.mode();
            if mode & 0o002 != 0 {
                return false;
            }
            true
        }
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_trusted_path(_path: &std::path::Path) -> bool {
    true
}

/// A single dependency entry — normalized from either string or object format.
#[derive(Debug, Clone)]
struct DepEntry {
    binary: String,
    version: Option<String>,
    package: String,
    apt_package: Option<String>,
    apk_package: Option<String>,
    manager: String,
    pip_name: Option<String>,
    uv_name: Option<String>,
    npm_name: Option<String>,
    use_npx: bool,
    fallback: Vec<FallbackEntry>,
}

/// A fallback install strategy.
#[derive(Debug, Clone)]
struct FallbackEntry {
    method: String,
    package: Option<String>,
    binary: Option<String>,
    source: Option<String>,
    manifest: Option<String>,
    features: Option<String>,
    url: Option<String>,
    args: Option<String>,
}

/// Per-tool dependency specification.
#[derive(Debug, Clone)]
struct ToolDepSpec {
    aliases: Vec<String>,
    required: Vec<DepEntry>,
    recommended: Vec<DepEntry>,
    config_files: Vec<String>,
    permissions: Vec<String>,
    network: Vec<String>,
}

/// Result of checking a single dependency item.
#[derive(Debug, Clone, PartialEq)]
enum DepStatus {
    Available,
    Missing,
    VersionLow { installed: String, required: String },
}

/// Overall readiness status for a tool.
#[derive(Debug, Clone, PartialEq)]
enum ReadyStatus {
    /// All required and recommended dependencies satisfied.
    Ready,
    /// Recommended deps missing but required deps OK — degraded but usable.
    Partial,
    /// Required deps or permissions missing — tool cannot function.
    NotReady,
    /// Tool not found in config dictionary — normal skip, no action needed.
    Unknown,
}

/// Combined result for a tool's environment check.
struct ToolReadyResult {
    tool_name: String,
    status: ReadyStatus,
    required_results: Vec<(DepEntry, DepStatus)>,
    recommended_results: Vec<(DepEntry, DepStatus)>,
    config_results: Vec<(String, bool)>,
    permission_results: Vec<(String, bool)>,
    network_results: Vec<(String, bool)>,
}

/// Normalize a JSON value (string or object) into a DepEntry.
/// String "jq" → DepEntry { binary: "jq", package: "jq", manager: "rpm" }
/// Object {binary, version, package, manager, ...} → DepEntry
fn normalize_dep(value: &Value) -> DepEntry {
    match value {
        Value::String(s) => {
            // Handle version constraints: "rtk>=0.35"
            if let Some(idx) = s.find(">=") {
                let binary = s[..idx].to_string();
                let version = Some(s[idx..].to_string());
                DepEntry {
                    binary,
                    version,
                    package: s[..idx].to_string(),
                    apt_package: None,
                    apk_package: None,
                    manager: "rpm".to_string(),
                    pip_name: None,
                    uv_name: None,
                    npm_name: None,
                    use_npx: false,
                    fallback: Vec::new(),
                }
            } else {
                DepEntry {
                    binary: s.clone(),
                    version: None,
                    package: s.clone(),
                    apt_package: None,
                    apk_package: None,
                    manager: "rpm".to_string(),
                    pip_name: None,
                    uv_name: None,
                    npm_name: None,
                    use_npx: false,
                    fallback: Vec::new(),
                }
            }
        }
        Value::Object(obj) => {
            let binary = obj
                .get("binary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let version = obj
                .get("version")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let package = obj
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(&binary)
                .to_string();
            let apt_package = obj
                .get("apt_package")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let apk_package = obj
                .get("apk_package")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let manager = obj
                .get("manager")
                .and_then(|v| v.as_str())
                .unwrap_or("rpm")
                .to_string();
            let pip_name = obj
                .get("pip_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let uv_name = obj
                .get("uv_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let npm_name = obj
                .get("npm_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let use_npx = obj
                .get("use_npx")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let fallback = obj
                .get("fallback")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|fb| {
                            if let Value::Object(fb_obj) = fb {
                                Some(FallbackEntry {
                                    method: fb_obj
                                        .get("method")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    package: fb_obj
                                        .get("package")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    binary: fb_obj
                                        .get("binary")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    source: fb_obj
                                        .get("source")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    manifest: fb_obj
                                        .get("manifest")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    features: fb_obj
                                        .get("features")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    url: fb_obj
                                        .get("url")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                    args: fb_obj
                                        .get("args")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string()),
                                })
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            DepEntry {
                binary,
                version,
                package,
                apt_package,
                apk_package,
                manager,
                pip_name,
                uv_name,
                npm_name,
                use_npx,
                fallback,
            }
        }
        _ => DepEntry {
            binary: "".to_string(),
            version: None,
            package: "".to_string(),
            apt_package: None,
            apk_package: None,
            manager: "rpm".to_string(),
            pip_name: None,
            uv_name: None,
            npm_name: None,
            use_npx: false,
            fallback: Vec::new(),
        },
    }
}

/// Normalize an array of dep values (strings or objects) into Vec<DepEntry>.
fn normalize_deps(array: &Value) -> Vec<DepEntry> {
    array
        .as_array()
        .map(|arr| arr.iter().map(normalize_dep).collect())
        .unwrap_or_default()
}

/// Detect the system's native package manager by checking the underlying
/// package management mechanism (rpm vs dpkg vs apk), then selecting the
/// best frontend within that family. Override via TOKENLESS_PACKAGE_MANAGER
/// env var (useful for testing).
fn detect_system_manager() -> String {
    if let Ok(mgr) = std::env::var("TOKENLESS_PACKAGE_MANAGER") {
        return mgr;
    }
    // Detect by underlying mechanism first, then pick frontend within family
    // rpm-based: prefer dnf (modern), then yum (legacy)
    // dpkg-based: apt-get
    // apk-based: apk
    let rpm_exists = Command::new("sh")
        .args(["-c", "command -v rpm"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let dpkg_exists = Command::new("sh")
        .args(["-c", "command -v dpkg"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let apk_exists = Command::new("sh")
        .args(["-c", "command -v apk"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if rpm_exists {
        // Pick best frontend: dnf (modern Fedora/RHEL 8+) > yum (legacy)
        if Command::new("sh")
            .args(["-c", "command -v dnf"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return "dnf".to_string();
        }
        return "yum".to_string();
    }
    if dpkg_exists {
        return "apt".to_string();
    }
    if apk_exists {
        return "apk".to_string();
    }
    "rpm".to_string()
}

/// Resolve a semantic manager label to the actual system package manager.
/// "rpm" maps to the detected system manager; other labels pass through unchanged.
fn resolve_manager(manager: &str) -> String {
    if manager == "rpm" {
        detect_system_manager()
    } else {
        manager.to_string()
    }
}

/// Resolve the actual package name for the detected system manager.
/// When manager="rpm" (meaning auto-detect), the detected manager may be apt/apk,
/// and those systems have different package names. apt_package/apk_package override
/// the default package field when present.
fn resolve_package(dep: &DepEntry) -> String {
    let detected = resolve_manager(&dep.manager);
    if dep.manager == "rpm" {
        match detected.as_str() {
            "apt" => dep
                .apt_package
                .as_deref()
                .unwrap_or(&dep.package)
                .to_string(),
            "apk" => dep
                .apk_package
                .as_deref()
                .unwrap_or(&dep.package)
                .to_string(),
            _ => dep.package.clone(),
        }
    } else {
        dep.package.clone()
    }
}

/// Extract the required version from a constraint string like ">=0.35".
fn extract_required_version(version: &str) -> &str {
    version
        .strip_prefix(">=")
        .or_else(|| version.strip_prefix(">"))
        .unwrap_or(version)
}

/// Compare version strings (semver-like: major.minor.patch).
/// Handles prefixed versions like "v22.1.0" and build suffixes like "1.2.3-rc1".
fn version_ge(installed: &str, required: &str) -> bool {
    fn parse_ver(s: &str) -> Vec<u32> {
        let cleaned = s
            .trim()
            .strip_prefix('v')
            .or_else(|| s.trim().strip_prefix('V'))
            .unwrap_or(s.trim());
        cleaned
            .split('.')
            .filter_map(|seg| {
                let num_part = seg
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
                    .unwrap_or("");
                num_part.parse().ok()
            })
            .collect()
    }
    let i_parts = parse_ver(installed);
    let r_parts = parse_ver(required);

    for i in 0..3 {
        let iv = i_parts.get(i).copied().unwrap_or(0);
        let rv = r_parts.get(i).copied().unwrap_or(0);
        if iv > rv {
            return true;
        }
        if iv < rv {
            return false;
        }
    }
    true
}

/// Check if a binary is available and meets version constraints.
fn check_dep(dep: &DepEntry) -> DepStatus {
    let which_result = Command::new("sh")
        .args(["-c", "command -v \"$1\"", "--", &dep.binary])
        .output();

    let binary_path: Option<String> = match which_result {
        Ok(output) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        _ => {
            // PATH lookup failed — try known install paths
            let home = super::get_home_dir();
            let candidates = [
                format!("/usr/libexec/anolisa/tokenless/{}", dep.binary),
                format!("/usr/lib/anolisa/tokenless/{}", dep.binary),
                format!("{}/.local/bin/{}", home, dep.binary),
                format!("{}/.local/lib/anolisa/tokenless/{}", home, dep.binary),
            ];
            candidates
                .iter()
                .find(|p| {
                    let path = std::path::Path::new(p);
                    path.exists()
                        && std::fs::metadata(path)
                            .map(|m| {
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::PermissionsExt;
                                    m.permissions().mode() & 0o111 != 0
                                }
                                #[cfg(not(unix))]
                                true
                            })
                            .unwrap_or(false)
                })
                .cloned()
        }
    };

    match binary_path {
        Some(path) if !path.is_empty() => {
            if let Some(ref version) = dep.version {
                let required_version = extract_required_version(version);
                let version_output = Command::new(&path).arg("--version").output();
                let installed_version = match version_output {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        stdout
                            .lines()
                            .next()
                            .unwrap_or("")
                            .split_whitespace()
                            .last()
                            .unwrap_or("0.0.0")
                            .to_string()
                    }
                    Err(_) => "0.0.0".to_string(),
                };

                if version_ge(&installed_version, required_version) {
                    DepStatus::Available
                } else {
                    DepStatus::VersionLow {
                        installed: installed_version,
                        required: required_version.to_string(),
                    }
                }
            } else {
                DepStatus::Available
            }
        }
        _ => DepStatus::Missing,
    }
}

/// Expand ~/... in paths to HOME directory.
fn expand_path(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        let home = super::get_home_dir();
        path.replacen("~", &home, 1)
    } else {
        path.to_string()
    }
}

/// Check if a config file exists.
fn check_config_file(path: &str) -> bool {
    let expanded = expand_path(path);
    fs::metadata(&expanded).is_ok()
}

/// Check a permission type.
fn check_permission(perm: &str) -> bool {
    match perm {
        "file_read" => fs::read_to_string("/etc/hostname").is_ok(),
        "file_write" => {
            let test_path =
                std::env::temp_dir().join(format!(".tokenless-ready-test-{}", std::process::id()));
            let can_write = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&test_path)
                .is_ok();
            if can_write {
                let _ = fs::remove_file(&test_path);
            }
            can_write
        }
        "exec_shell" => Command::new("sh")
            .args(["-c", "command -v bash"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        _ => true,
    }
}

/// Check network connectivity.
fn check_network(net: &str) -> bool {
    match net {
        "https_outbound" => Command::new("curl")
            .args(["-s", "--max-time", "2", "https://example.com"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        _ => true,
    }
}

/// Load tool-ready-spec.json with both string and object format support.
fn load_spec(
    spec_path: &PathBuf,
) -> Result<std::collections::HashMap<String, ToolDepSpec>, String> {
    let content =
        fs::read_to_string(spec_path).map_err(|e| format!("Failed to read spec file: {}", e))?;
    let value: Value =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse spec JSON: {}", e))?;

    let mut specs = std::collections::HashMap::new();
    // Skip _comment key
    if let Value::Object(obj) = value {
        for (tool_name, tool_spec) in obj {
            if tool_name.starts_with('_') {
                continue;
            }
            if let Value::Object(spec_obj) = tool_spec {
                let aliases = spec_obj
                    .get("aliases")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let required = normalize_deps(
                    spec_obj
                        .get("required")
                        .unwrap_or(&Value::Array(Vec::new())),
                );
                let recommended = normalize_deps(
                    spec_obj
                        .get("recommended")
                        .unwrap_or(&Value::Array(Vec::new())),
                );
                let config_files = spec_obj
                    .get("config_files")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let permissions = spec_obj
                    .get("permissions")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let network = spec_obj
                    .get("network")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                specs.insert(
                    tool_name,
                    ToolDepSpec {
                        aliases,
                        required,
                        recommended,
                        config_files,
                        permissions,
                        network,
                    },
                );
            }
        }
    }
    Ok(specs)
}

/// Check a specific tool's environment readiness.
fn check_tool(tool_name: &str, spec: &ToolDepSpec) -> ToolReadyResult {
    let required_results: Vec<(DepEntry, DepStatus)> = spec
        .required
        .iter()
        .map(|d| (d.clone(), check_dep(d)))
        .collect();

    let recommended_results: Vec<(DepEntry, DepStatus)> = spec
        .recommended
        .iter()
        .map(|d| (d.clone(), check_dep(d)))
        .collect();

    let config_results: Vec<(String, bool)> = spec
        .config_files
        .iter()
        .map(|f| (f.clone(), check_config_file(f)))
        .collect();

    let permission_results: Vec<(String, bool)> = spec
        .permissions
        .iter()
        .map(|p| (p.clone(), check_permission(p)))
        .collect();

    let network_results: Vec<(String, bool)> = spec
        .network
        .iter()
        .map(|n| (n.clone(), check_network(n)))
        .collect();

    let has_required_missing = required_results
        .iter()
        .any(|(_, s)| s == &DepStatus::Missing || matches!(s, DepStatus::VersionLow { .. }));
    let has_perm_missing = permission_results.iter().any(|(_, ok)| !ok);
    let has_recommended_missing = recommended_results
        .iter()
        .any(|(_, s)| s == &DepStatus::Missing);
    let has_config_missing = config_results.iter().any(|(_, ok)| !ok);
    let has_net_missing = network_results.iter().any(|(_, ok)| !ok);

    let status = if has_required_missing || has_perm_missing {
        ReadyStatus::NotReady
    } else if has_recommended_missing || has_config_missing || has_net_missing {
        ReadyStatus::Partial
    } else {
        ReadyStatus::Ready
    };

    ToolReadyResult {
        tool_name: tool_name.to_string(),
        status,
        required_results,
        recommended_results,
        config_results,
        permission_results,
        network_results,
    }
}

/// Format a DepStatus as a human-readable string (with icon).
fn format_dep_status(status: &DepStatus) -> String {
    match status {
        DepStatus::Available => "✓".to_string(),
        DepStatus::Missing => "missing".to_string(),
        DepStatus::VersionLow {
            installed,
            required,
        } => {
            format!("version low ({} < {})", installed, required)
        }
    }
}

/// Format a DepStatus as a text status label (no icon, no emoji).
fn format_dep_status_label(status: &DepStatus) -> String {
    match status {
        DepStatus::Available => "INSTALLED".to_string(),
        DepStatus::Missing => "MISSING".to_string(),
        DepStatus::VersionLow {
            installed,
            required,
        } => {
            format!("OUTDATED ({}/{})", installed, required)
        }
    }
}

/// Format a ReadyStatus as a human-readable label.
fn format_status(status: &ReadyStatus) -> &'static str {
    match status {
        ReadyStatus::Ready => "READY",
        ReadyStatus::Partial => "PARTIAL",
        ReadyStatus::NotReady => "NOT_READY",
        ReadyStatus::Unknown => "UNKNOWN",
    }
}

/// Generate a full checklist string — two-level layout:
/// Level 1: Agent tool category (Shell/WebFetch/Read/Write)
/// Level 2: Binary list under each category, with text status labels.
fn generate_checklist(results: &[ToolReadyResult]) -> String {
    let mut output = String::new();
    output.push_str("Tool Environment Ready Checklist\n");
    output.push_str("=================================\n\n");

    for result in results {
        let category_status = format_status(&result.status);
        output.push_str(&format!("{} [{}]\n", result.tool_name, category_status));

        for (dep, status) in &result.required_results {
            let label = format_dep_status_label(status);
            output.push_str(&format!("  required:   {:12} {}\n", dep.binary, label));
        }
        for (dep, status) in &result.recommended_results {
            let label = format_dep_status_label(status);
            output.push_str(&format!("  recommended:{:12} {}\n", dep.binary, label));
        }
        for (cfg, ok) in &result.config_results {
            let label = if *ok { "INSTALLED" } else { "MISSING" };
            output.push_str(&format!("  config:     {:12} {}\n", cfg, label));
        }
        for (perm, ok) in &result.permission_results {
            let label = if *ok { "GRANTED" } else { "DENIED" };
            output.push_str(&format!("  permission: {:12} {}\n", perm, label));
        }
        if !result.required_results.is_empty()
            || !result.recommended_results.is_empty()
            || !result.config_results.is_empty()
            || !result.permission_results.is_empty()
        {
            output.push('\n');
        }
    }

    let ready_count = results
        .iter()
        .filter(|r| r.status == ReadyStatus::Ready)
        .count();
    let partial_count = results
        .iter()
        .filter(|r| r.status == ReadyStatus::Partial)
        .count();
    let not_ready_count = results
        .iter()
        .filter(|r| r.status == ReadyStatus::NotReady)
        .count();
    let unknown_count = results
        .iter()
        .filter(|r| r.status == ReadyStatus::Unknown)
        .count();

    let mut summary = format!(
        "Summary: {} ready, {} partial, {} not ready",
        ready_count, partial_count, not_ready_count
    );
    if unknown_count > 0 {
        summary.push_str(&format!(", {} unknown", unknown_count));
    }
    summary.push_str(&format!(" (total: {})\n", results.len()));
    output.push_str(&summary);

    output
}

/// Auto-fix missing dependencies via tokenless-env-fix.sh.
fn auto_fix(missing_deps: &[DepEntry]) -> Result<String, String> {
    let home = super::get_home_dir();
    let fix_script_env = std::env::var("TOKENLESS_ENV_FIX_SCRIPT").ok();
    let fix_script_candidates = [
        fix_script_env,
        Some(format!("{}/.tokenless/tokenless-env-fix.sh", home)),
        Some(format!(
            "{}/.local/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh",
            home
        )),
        Some("/usr/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh".to_string()),
        // Legacy paths (pre-FHS refactor, flat layout without common/ subdir)
        Some(format!(
            "{}/.local/share/anolisa/adapters/tokenless/tokenless-env-fix.sh",
            home
        )),
        Some("/usr/share/anolisa/adapters/tokenless/tokenless-env-fix.sh".to_string()),
    ];
    let fix_script = fix_script_candidates
        .iter()
        .flatten()
        .find(|p| {
            let path = std::path::Path::new(p);
            path.exists() && is_trusted_path(path)
        })
        .cloned()
        .unwrap_or_else(|| format!("{}/.tokenless/tokenless-env-fix.sh", home));

    // Build JSON array of missing deps
    let deps_json: Vec<Value> = missing_deps
        .iter()
        .map(|dep| {
            let mut obj = serde_json::Map::new();
            obj.insert("binary".to_string(), Value::String(dep.binary.clone()));
            if let Some(ref v) = dep.version {
                obj.insert("version".to_string(), Value::String(v.clone()));
            }
            obj.insert("package".to_string(), Value::String(resolve_package(dep)));
            if let Some(ref ap) = dep.apt_package {
                obj.insert("apt_package".to_string(), Value::String(ap.clone()));
            }
            if let Some(ref akp) = dep.apk_package {
                obj.insert("apk_package".to_string(), Value::String(akp.clone()));
            }
            obj.insert("manager".to_string(), Value::String(dep.manager.clone()));
            if let Some(ref pn) = dep.pip_name {
                obj.insert("pip_name".to_string(), Value::String(pn.clone()));
            }
            if let Some(ref un) = dep.uv_name {
                obj.insert("uv_name".to_string(), Value::String(un.clone()));
            }
            if let Some(ref nn) = dep.npm_name {
                obj.insert("npm_name".to_string(), Value::String(nn.clone()));
            }
            if dep.use_npx {
                obj.insert("use_npx".to_string(), Value::Bool(true));
            }
            if !dep.fallback.is_empty() {
                let fb_arr: Vec<Value> = dep
                    .fallback
                    .iter()
                    .map(|fb| {
                        let mut fb_obj = serde_json::Map::new();
                        fb_obj.insert("method".to_string(), Value::String(fb.method.clone()));
                        if let Some(ref p) = fb.package {
                            fb_obj.insert("package".to_string(), Value::String(p.clone()));
                        }
                        if let Some(ref b) = fb.binary {
                            fb_obj.insert("binary".to_string(), Value::String(b.clone()));
                        }
                        if let Some(ref s) = fb.source {
                            fb_obj.insert("source".to_string(), Value::String(s.clone()));
                        }
                        if let Some(ref m) = fb.manifest {
                            fb_obj.insert("manifest".to_string(), Value::String(m.clone()));
                        }
                        if let Some(ref f) = fb.features {
                            fb_obj.insert("features".to_string(), Value::String(f.clone()));
                        }
                        if let Some(ref u) = fb.url {
                            fb_obj.insert("url".to_string(), Value::String(u.clone()));
                        }
                        if let Some(ref a) = fb.args {
                            fb_obj.insert("args".to_string(), Value::String(a.clone()));
                        }
                        Value::Object(fb_obj)
                    })
                    .collect();
                obj.insert("fallback".to_string(), Value::Array(fb_arr));
            }
            Value::Object(obj)
        })
        .collect();

    let json_str = serde_json::to_string(&deps_json)
        .map_err(|e| format!("Failed to serialize deps: {}", e))?;

    // Use timeout if available (coreutils procps), otherwise run without timeout
    let has_timeout = Command::new("sh")
        .args(["-c", "command -v timeout"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut child = if has_timeout {
        Command::new("timeout")
            .arg("120")
            .arg("bash")
            .arg(&fix_script)
            .arg("fix-all")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run env-fix: {}", e))?
    } else {
        Command::new("bash")
            .arg(&fix_script)
            .arg("fix-all")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run env-fix: {}", e))?
    };

    let mut stdin_handle = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open stdin for env-fix process".to_string())?;
    stdin_handle
        .write_all(json_str.as_bytes())
        .map_err(|e| format!("Failed to write deps to env-fix stdin: {}", e))?;
    drop(stdin_handle);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait for env-fix: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Surface stderr (and stdout) so the caller can show the failure
        // instead of silently treating an error message as a "success" payload.
        return Err(format!(
            "env-fix exited with {}: {}{}",
            output.status,
            stderr.trim(),
            if stdout.is_empty() {
                String::new()
            } else {
                format!(" | stdout: {}", stdout.trim())
            }
        ));
    }
    Ok(stdout)
}

/// Find the spec file path.
fn find_spec_path() -> Result<PathBuf, String> {
    let home = super::get_home_dir();
    let candidates = [
        std::env::var("TOKENLESS_TOOL_READY_SPEC")
            .ok()
            .map(PathBuf::from),
        Some(PathBuf::from(format!(
            "{}/.tokenless/tool-ready-spec.json",
            home
        ))),
        Some(PathBuf::from(format!(
            "{}/.local/share/anolisa/adapters/tokenless/common/tool-ready-spec.json",
            home
        ))),
        Some(PathBuf::from(
            "/usr/share/anolisa/adapters/tokenless/common/tool-ready-spec.json",
        )),
        // Legacy paths (pre-FHS refactor, flat layout without common/ subdir)
        Some(PathBuf::from(format!(
            "{}/.local/share/anolisa/adapters/tokenless/tool-ready-spec.json",
            home
        ))),
        Some(PathBuf::from(
            "/usr/share/anolisa/adapters/tokenless/tool-ready-spec.json",
        )),
    ];

    for candidate in candidates.iter().flatten() {
        if candidate.exists() && is_trusted_path(candidate) {
            return Ok(candidate.clone());
        }
    }

    let candidate_list: Vec<String> = candidates
        .iter()
        .filter_map(|c| c.as_ref().map(|p| p.display().to_string()))
        .collect();
    Err(format!(
        "No spec file found in any candidate path: {}",
        candidate_list.join(", ")
    ))
}

/// Build a JSON result for a single tool check.
fn build_json_result(
    tool_name: &str,
    status: &ReadyStatus,
    fixed: &[String],
    missing: &[String],
) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("tool".to_string(), Value::String(tool_name.to_string()));
    obj.insert(
        "status".to_string(),
        Value::String(format_status(status).to_string()),
    );
    if !fixed.is_empty() {
        obj.insert(
            "fixed".to_string(),
            Value::Array(fixed.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    if !missing.is_empty() {
        obj.insert(
            "missing".to_string(),
            Value::Array(missing.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    if *status == ReadyStatus::NotReady {
        let diag_parts: Vec<String> = missing
            .iter()
            .map(|m| format!("required dependency missing: {}", m))
            .collect();
        obj.insert(
            "diagnostic".to_string(),
            Value::String(format!(
                "[tokenless:ready] {}: NOT_READY — {}. Skip retry.",
                tool_name,
                diag_parts.join(", ")
            )),
        );
    }
    Value::Object(obj)
}

/// Run the env-check command with optional JSON output.
pub fn run(
    tool: Option<&str>,
    all: bool,
    fix: bool,
    checklist: bool,
    json: bool,
) -> Result<(), (String, i32)> {
    let spec_path = find_spec_path().map_err(|e| (e, 1))?;
    let specs = load_spec(&spec_path).map_err(|e| (e, 1))?;

    if checklist {
        let results: Vec<ToolReadyResult> = specs
            .keys()
            .map(|name| check_tool(name, specs.get(name).unwrap()))
            .collect();
        println!("{}", generate_checklist(&results));
        return Ok(());
    }

    let tool_names: Vec<String> = if all {
        specs.keys().cloned().collect()
    } else if let Some(t) = tool {
        // Resolve tool name: exact key → aliases → case-insensitive
        let resolved = if specs.contains_key(t) {
            t.to_string()
        } else {
            // Try alias reverse lookup: find spec key whose aliases contain t
            specs
                .iter()
                .find(|(_, spec)| spec.aliases.iter().any(|a| a == t))
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| {
                    // Case-insensitive fallback
                    specs
                        .keys()
                        .find(|k| k.eq_ignore_ascii_case(t))
                        .cloned()
                        .unwrap_or_else(|| t.to_string())
                })
        };
        if !specs.contains_key(&resolved) {
            if json {
                let result = build_json_result(&resolved, &ReadyStatus::Unknown, &[], &[]);
                println!("{}", serde_json::to_string(&result).unwrap());
                return Ok(());
            }
            println!("{}: {}", t, format_status(&ReadyStatus::Unknown));
            return Ok(());
        }
        vec![resolved]
    } else {
        return Err(("Specify --tool <name> or --all".to_string(), 1));
    };

    for tool_name in &tool_names {
        let spec = specs.get(tool_name).unwrap();
        let result = check_tool(tool_name, spec);

        // Collect missing and version-low deps for auto-fix
        let missing_deps: Vec<DepEntry> = result
            .required_results
            .iter()
            .chain(result.recommended_results.iter())
            .filter(|(_, s)| matches!(s, DepStatus::Missing | DepStatus::VersionLow { .. }))
            .map(|(d, _)| d.clone())
            .collect();

        let missing_names: Vec<String> = missing_deps.iter().map(|d| d.binary.clone()).collect();

        if fix && !missing_deps.is_empty() {
            if !json {
                println!(
                    "{}: {} (fixing: {})",
                    tool_name,
                    format_status(&result.status),
                    missing_names.join(", ")
                );
                println!("  Attempting auto-fix...");
            }
            let fix_output = auto_fix(&missing_deps).map_err(|e| (e, 1))?;
            if !json {
                for line in fix_output.lines() {
                    println!("  {}", line);
                }
            }

            // Re-check after fix
            let post_result = check_tool(tool_name, spec);
            let post_missing: Vec<String> = post_result
                .required_results
                .iter()
                .chain(post_result.recommended_results.iter())
                .filter(|(_, s)| matches!(s, DepStatus::Missing | DepStatus::VersionLow { .. }))
                .map(|(d, _)| d.binary.clone())
                .collect();

            let fixed: Vec<String> = missing_names
                .iter()
                .filter(|n| !post_missing.contains(n))
                .cloned()
                .collect();

            if json {
                let post_status =
                    if post_missing.is_empty()
                        && post_result.permission_results.iter().all(|(_, ok)| *ok)
                    {
                        ReadyStatus::Ready
                    } else if post_result.required_results.iter().any(|(_, s)| {
                        matches!(s, DepStatus::Missing | DepStatus::VersionLow { .. })
                    }) || post_result.permission_results.iter().any(|(_, ok)| !ok)
                    {
                        ReadyStatus::NotReady
                    } else {
                        ReadyStatus::Partial
                    };
                let result_json = build_json_result(tool_name, &post_status, &fixed, &post_missing);
                println!("{}", serde_json::to_string(&result_json).unwrap());
            } else {
                println!("{}: {}", tool_name, format_status(&post_result.status));
            }
        } else if json {
            let result_json = build_json_result(tool_name, &result.status, &[], &missing_names);
            println!("{}", serde_json::to_string(&result_json).unwrap());
        } else {
            println!("{}: {}", tool_name, format_status(&result.status));

            for (dep, status) in &result.required_results {
                println!(
                    "  required: {} — {} [{}]",
                    dep.binary,
                    format_dep_status(status),
                    resolve_manager(&dep.manager)
                );
            }
            for (dep, status) in &result.recommended_results {
                println!(
                    "  recommended: {} — {} [{}]",
                    dep.binary,
                    format_dep_status(status),
                    resolve_manager(&dep.manager)
                );
            }
            for (cfg, ok) in &result.config_results {
                println!("  config: {} — {}", cfg, if *ok { "✓" } else { "missing" });
            }
            for (perm, ok) in &result.permission_results {
                println!(
                    "  permission: {} — {}",
                    perm,
                    if *ok { "✓" } else { "missing" }
                );
            }
            for (net, ok) in &result.network_results {
                println!("  network: {} — {}", net, if *ok { "✓" } else { "missing" });
            }

            if !missing_deps.is_empty() {
                println!(
                    "  Hint: run with --fix to auto-install missing deps: {}",
                    missing_deps
                        .iter()
                        .map(|d| d.binary.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }

        if !json {
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_dep_simple_string() {
        let dep = normalize_dep(&json!("jq"));
        assert_eq!(dep.binary, "jq");
        assert_eq!(dep.package, "jq");
        assert_eq!(dep.manager, "rpm");
        assert!(dep.version.is_none());
        assert!(dep.fallback.is_empty());
    }

    #[test]
    fn normalize_dep_version_string() {
        let dep = normalize_dep(&json!("rtk>=0.35"));
        assert_eq!(dep.binary, "rtk");
        assert_eq!(dep.version.as_deref(), Some(">=0.35"));
        assert_eq!(dep.package, "rtk");
        assert_eq!(dep.manager, "rpm");
    }

    #[test]
    fn normalize_dep_object() {
        let dep = normalize_dep(&json!({
            "binary": "curl",
            "package": "curl",
            "manager": "rpm"
        }));
        assert_eq!(dep.binary, "curl");
        assert_eq!(dep.package, "curl");
        assert_eq!(dep.manager, "rpm");
        assert!(dep.version.is_none());
    }

    #[test]
    fn normalize_dep_object_with_all_fields() {
        let dep = normalize_dep(&json!({
            "binary": "rtk",
            "version": ">=0.35",
            "package": "rtk",
            "manager": "cargo",
            "pip_name": "rtk-pip",
            "uv_name": "rtk-uv",
            "npm_name": "rtk-npm",
            "use_npx": true,
            "fallback": [
                {"method": "symlink", "binary": "rtk", "source": "/usr/libexec/anolisa/tokenless/rtk"}
            ]
        }));
        assert_eq!(dep.binary, "rtk");
        assert_eq!(dep.version.as_deref(), Some(">=0.35"));
        assert_eq!(dep.manager, "cargo");
        assert_eq!(dep.pip_name.as_deref(), Some("rtk-pip"));
        assert_eq!(dep.uv_name.as_deref(), Some("rtk-uv"));
        assert_eq!(dep.npm_name.as_deref(), Some("rtk-npm"));
        assert!(dep.use_npx);
        assert_eq!(dep.fallback.len(), 1);
        assert_eq!(dep.fallback[0].method, "symlink");
        assert_eq!(
            dep.fallback[0].source.as_deref(),
            Some("/usr/libexec/anolisa/tokenless/rtk")
        );
    }

    #[test]
    fn normalize_dep_null_fallback() {
        let dep = normalize_dep(&json!(null));
        assert_eq!(dep.binary, "");
        assert_eq!(dep.package, "");
        assert_eq!(dep.manager, "rpm");
    }

    #[test]
    fn normalize_deps_mixed_array() {
        let deps = normalize_deps(
            &json!(["jq", "rtk>=0.35", {"binary": "curl", "package": "curl", "manager": "rpm"}]),
        );
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].binary, "jq");
        assert_eq!(deps[0].manager, "rpm");
        assert_eq!(deps[1].binary, "rtk");
        assert_eq!(deps[1].version.as_deref(), Some(">=0.35"));
        assert_eq!(deps[2].binary, "curl");
        assert_eq!(deps[2].manager, "rpm");
    }

    #[test]
    fn normalize_deps_empty() {
        let deps = normalize_deps(&json!([]));
        assert!(deps.is_empty());
        let deps = normalize_deps(&json!(null));
        assert!(deps.is_empty());
    }

    #[test]
    fn extract_required_version_ge() {
        assert_eq!(extract_required_version(">=0.35"), "0.35");
    }

    #[test]
    fn extract_required_version_gt() {
        assert_eq!(extract_required_version(">1.0"), "1.0");
    }

    #[test]
    fn extract_required_version_no_operator() {
        assert_eq!(extract_required_version("0.35"), "0.35");
    }

    #[test]
    fn version_ge_equal() {
        assert!(version_ge("0.35", "0.35"));
    }

    #[test]
    fn version_ge_greater() {
        assert!(version_ge("1.2.0", "1.0.0"));
    }

    #[test]
    fn version_ge_less() {
        assert!(!version_ge("0.34", "0.35"));
    }

    #[test]
    fn version_ge_short_version() {
        assert!(version_ge("2.0", "1.9.9"));
    }

    #[test]
    fn version_ge_patch_comparison() {
        assert!(version_ge("1.0.1", "1.0.0"));
        assert!(!version_ge("1.0.0", "1.0.1"));
    }

    #[test]
    fn build_json_result_ready() {
        let result = build_json_result("Shell", &ReadyStatus::Ready, &[], &[]);
        assert_eq!(result["tool"], "Shell");
        assert_eq!(result["status"], "READY");
        assert!(result.get("fixed").is_none());
        assert!(result.get("missing").is_none());
        assert!(result.get("diagnostic").is_none());
    }

    #[test]
    fn build_json_result_not_ready() {
        let result = build_json_result(
            "Shell",
            &ReadyStatus::NotReady,
            &[],
            &["fakebin99".to_string()],
        );
        assert_eq!(result["tool"], "Shell");
        assert_eq!(result["status"], "NOT_READY");
        assert_eq!(result["missing"][0], "fakebin99");
        let diag = result["diagnostic"].as_str().unwrap();
        assert!(diag.contains("Skip retry"));
        assert!(diag.contains("required dependency missing"));
    }

    #[test]
    fn build_json_result_unknown() {
        let result = build_json_result("UnknownTool", &ReadyStatus::Unknown, &[], &[]);
        assert_eq!(result["tool"], "UnknownTool");
        assert_eq!(result["status"], "UNKNOWN");
        assert!(result.get("fixed").is_none());
        assert!(result.get("missing").is_none());
        assert!(result.get("diagnostic").is_none());
    }

    #[test]
    fn build_json_result_with_fixed() {
        let result = build_json_result("Shell", &ReadyStatus::Ready, &["jq".to_string()], &[]);
        assert_eq!(result["fixed"][0], "jq");
    }

    #[test]
    fn format_status_all() {
        assert_eq!(format_status(&ReadyStatus::Ready), "READY");
        assert_eq!(format_status(&ReadyStatus::Partial), "PARTIAL");
        assert_eq!(format_status(&ReadyStatus::NotReady), "NOT_READY");
        assert_eq!(format_status(&ReadyStatus::Unknown), "UNKNOWN");
    }

    #[test]
    fn format_dep_status_all() {
        assert_eq!(format_dep_status(&DepStatus::Available), "✓");
        assert_eq!(format_dep_status(&DepStatus::Missing), "missing");
        let low = format_dep_status(&DepStatus::VersionLow {
            installed: "0.34".to_string(),
            required: "0.35".to_string(),
        });
        assert!(low.contains("0.34"));
        assert!(low.contains("0.35"));
    }

    #[test]
    fn expand_path_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let expanded = expand_path("~/.copilot-shell/settings.json");
        assert_eq!(expanded, format!("{}/.copilot-shell/settings.json", home));
    }

    #[test]
    fn expand_path_absolute() {
        let expanded = expand_path("/etc/config.json");
        assert_eq!(expanded, "/etc/config.json");
    }

    #[test]
    fn version_ge_prefixed_v() {
        assert!(version_ge("v22.1.0", "16.0.0"));
        assert!(version_ge("V22.1.0", "16.0.0"));
    }

    #[test]
    fn version_ge_build_suffix() {
        assert!(version_ge("1.2.3-rc1", "1.2.0"));
        assert!(version_ge("1.2.3+build", "1.2.3"));
    }

    #[test]
    fn version_ge_short_segments() {
        assert!(version_ge("22.1", "16.0"));
        assert!(!version_ge("1.0", "2.0"));
    }

    #[test]
    fn load_spec_skips_meta_keys() {
        let tmp_dir = std::env::temp_dir();
        let spec_path = tmp_dir.join("test-tool-ready-spec.json");
        let spec_content = json!({
            "_meta": {"version": "2.0"},
            "_comment": "this should be skipped",
            "Shell": {
                "required": ["jq"],
                "recommended": [],
                "config_files": [],
                "permissions": [],
                "network": []
            }
        });
        std::fs::write(&spec_path, serde_json::to_string(&spec_content).unwrap()).unwrap();

        let specs = load_spec(&spec_path).unwrap();
        assert!(!specs.contains_key("_meta"));
        assert!(!specs.contains_key("_comment"));
        assert!(specs.contains_key("Shell"));
        let shell_spec = specs.get("Shell").unwrap();
        assert_eq!(shell_spec.required.len(), 1);
        assert_eq!(shell_spec.required[0].binary, "jq");

        std::fs::remove_file(&spec_path).ok();
    }

    #[test]
    fn load_spec_mixed_formats() {
        let tmp_dir = std::env::temp_dir();
        let spec_path = tmp_dir.join("test-mixed-spec.json");
        let spec_content = json!({
            "Shell": {
                "required": ["jq", "rtk>=0.35", {"binary": "curl", "package": "curl", "manager": "rpm"}],
                "recommended": [],
                "config_files": [],
                "permissions": [],
                "network": []
            }
        });
        std::fs::write(&spec_path, serde_json::to_string(&spec_content).unwrap()).unwrap();

        let specs = load_spec(&spec_path).unwrap();
        let shell_spec = specs.get("Shell").unwrap();
        assert_eq!(shell_spec.required.len(), 3);
        assert_eq!(shell_spec.required[0].binary, "jq");
        assert_eq!(shell_spec.required[0].manager, "rpm");
        assert_eq!(shell_spec.required[1].binary, "rtk");
        assert_eq!(shell_spec.required[1].version.as_deref(), Some(">=0.35"));
        assert_eq!(shell_spec.required[2].binary, "curl");
        assert_eq!(shell_spec.required[2].manager, "rpm");

        std::fs::remove_file(&spec_path).ok();
    }

    #[test]
    fn generate_checklist_unknown_status() {
        let results = [ToolReadyResult {
            tool_name: "UnknownTool".to_string(),
            status: ReadyStatus::Unknown,
            required_results: vec![(
                DepEntry {
                    binary: "fake".to_string(),
                    version: None,
                    package: "fake".to_string(),
                    apt_package: None,
                    apk_package: None,
                    manager: "rpm".to_string(),
                    pip_name: None,
                    uv_name: None,
                    npm_name: None,
                    use_npx: false,
                    fallback: vec![],
                },
                DepStatus::Missing,
            )],
            recommended_results: vec![],
            config_results: vec![],
            permission_results: vec![],
            network_results: vec![],
        }];
        let checklist = generate_checklist(&results);
        assert!(checklist.contains("UNKNOWN"));
        assert!(checklist.contains("unknown"));
    }
}
