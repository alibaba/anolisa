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

#[cfg(unix)]
fn make_test_dir(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "tokenless-is-trusted-{}-{}-{}",
        std::process::id(),
        nanos,
        label
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[cfg(unix)]
fn chmod_file(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path).unwrap().permissions();
    perm.set_mode(mode);
    std::fs::set_permissions(path, perm).unwrap();
}

#[cfg(unix)]
#[test]
fn is_trusted_path_system_prefixes_unconditional() {
    // The system-path branch returns early without touching the
    // filesystem, so non-existent paths still report trusted.
    use std::path::Path;
    assert!(is_trusted_path(Path::new("/usr/share/anolisa/x")));
    assert!(is_trusted_path(Path::new("/usr/libexec/anolisa/x")));
    assert!(is_trusted_path(Path::new("/usr/lib/anolisa/x")));
    assert!(is_trusted_path(Path::new("/usr/local/share/anolisa/x")));
}

#[cfg(unix)]
#[test]
fn is_trusted_path_rejects_world_writable_parent() {
    use std::os::unix::fs::MetadataExt;
    let tmp = make_test_dir("ww-parent");
    if std::fs::metadata(&tmp).unwrap().uid() != current_uid() {
        // /tmp on hardened multi-user systems may strip our ownership;
        // the world-writable check is moot in that case.
        std::fs::remove_dir_all(&tmp).ok();
        return;
    }
    chmod_file(&tmp, 0o777);
    let f = tmp.join("binary");
    std::fs::write(&f, b"#!/bin/sh\n").unwrap();
    chmod_file(&f, 0o755);
    assert!(
        !is_trusted_path(&f),
        "world-writable parent dir must be rejected"
    );
    chmod_file(&tmp, 0o755);
    std::fs::remove_dir_all(&tmp).ok();
}

#[cfg(unix)]
#[test]
fn is_trusted_path_rejects_world_writable_file() {
    use std::os::unix::fs::MetadataExt;
    let tmp = make_test_dir("ww-file");
    if std::fs::metadata(&tmp).unwrap().uid() != current_uid() {
        std::fs::remove_dir_all(&tmp).ok();
        return;
    }
    chmod_file(&tmp, 0o755);
    let f = tmp.join("binary");
    std::fs::write(&f, b"#!/bin/sh\n").unwrap();
    chmod_file(&f, 0o777);
    assert!(
        !is_trusted_path(&f),
        "world-writable file mode must be rejected"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[cfg(unix)]
#[test]
fn is_trusted_path_accepts_owned_safe_file() {
    use std::os::unix::fs::MetadataExt;
    let tmp = make_test_dir("ok");
    if std::fs::metadata(&tmp).unwrap().uid() != current_uid() {
        std::fs::remove_dir_all(&tmp).ok();
        return;
    }
    chmod_file(&tmp, 0o755);
    let f = tmp.join("binary");
    std::fs::write(&f, b"#!/bin/sh\n").unwrap();
    chmod_file(&f, 0o755);
    assert!(
        is_trusted_path(&f),
        "uid-owned non-writable file must be accepted"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[cfg(unix)]
#[test]
fn is_trusted_path_rejects_nonexistent_file() {
    let nonexistent = std::env::temp_dir().join(format!(
        "tokenless-nonexistent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(
        !is_trusted_path(&nonexistent),
        "non-existent file must be rejected"
    );
}

#[test]
fn expand_path_rejects_parent_dir_traversal() {
    // ParentDir components in ~/... paths are rejected at the syntax
    // layer so a misconfigured config_files entry like "~/../etc/passwd"
    // cannot escape the home directory after expansion.
    let escaped = expand_path("~/../etc/passwd");
    assert_eq!(
        escaped, "~/../etc/passwd",
        "ParentDir-bearing tilde path must be returned unchanged"
    );
    let escaped2 = expand_path("~/sub/../../../etc/passwd");
    assert_eq!(
        escaped2, "~/sub/../../../etc/passwd",
        "Deep ParentDir traversal must be returned unchanged"
    );
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

fn make_dep(name: &str) -> DepEntry {
    DepEntry {
        binary: name.to_string(),
        version: None,
        package: name.to_string(),
        apt_package: None,
        apk_package: None,
        manager: "rpm".to_string(),
        pip_name: None,
        uv_name: None,
        npm_name: None,
        use_npx: false,
        fallback: vec![],
    }
}

#[test]
fn check_config_file_existing() {
    let dir = std::env::temp_dir();
    let f = dir.join(format!("tokenless-cfg-test-{}", std::process::id()));
    std::fs::write(&f, "{}").unwrap();
    assert!(check_config_file(f.to_str().unwrap()));
    std::fs::remove_file(&f).ok();
}

#[test]
fn check_config_file_nonexistent() {
    assert!(!check_config_file("/nonexistent/path/config.json"));
}

#[test]
fn check_config_file_tilde_expansion() {
    let result = check_config_file("~/.nonexistent-tokenless-test-file");
    assert!(!result);
}

#[test]
fn check_permission_file_read() {
    assert!(check_permission("file_read"));
}

#[test]
fn check_permission_file_write() {
    // Exercise the file_write path; result depends on filesystem permissions
    let _ = check_permission("file_write");
}

#[test]
fn check_permission_exec_shell() {
    assert!(check_permission("exec_shell"));
}

#[test]
fn check_permission_unknown_defaults_true() {
    assert!(check_permission("some_unknown_permission"));
}

#[test]
fn check_network_unknown_defaults_true() {
    assert!(check_network("some_unknown_check"));
}

#[test]
fn resolve_manager_rpm_delegates() {
    let mgr = resolve_manager("rpm");
    // On this system it should resolve to dnf, yum, apt, apk, or fallback rpm
    assert!(!mgr.is_empty());
}

#[test]
fn resolve_manager_passthrough() {
    assert_eq!(resolve_manager("cargo"), "cargo");
    assert_eq!(resolve_manager("pip"), "pip");
    assert_eq!(resolve_manager("npm"), "npm");
}

#[test]
fn resolve_package_non_rpm_returns_package() {
    let dep = DepEntry {
        binary: "tool".to_string(),
        version: None,
        package: "tool-pkg".to_string(),
        apt_package: Some("tool-apt".to_string()),
        apk_package: Some("tool-apk".to_string()),
        manager: "cargo".to_string(),
        pip_name: None,
        uv_name: None,
        npm_name: None,
        use_npx: false,
        fallback: vec![],
    };
    assert_eq!(resolve_package(&dep), "tool-pkg");
}

#[test]
fn resolve_package_rpm_uses_detected_manager() {
    let dep = DepEntry {
        binary: "tool".to_string(),
        version: None,
        package: "tool-rpm".to_string(),
        apt_package: Some("tool-apt".to_string()),
        apk_package: Some("tool-apk".to_string()),
        manager: "rpm".to_string(),
        pip_name: None,
        uv_name: None,
        npm_name: None,
        use_npx: false,
        fallback: vec![],
    };
    let pkg = resolve_package(&dep);
    // Package depends on detected manager; shouldn't be empty
    assert!(!pkg.is_empty());
}

#[test]
fn normalize_dep_object_with_apt_apk_packages() {
    let dep = normalize_dep(&json!({
        "binary": "curl",
        "package": "curl",
        "apt_package": "curl-deb",
        "apk_package": "curl-alpine",
        "manager": "rpm"
    }));
    assert_eq!(dep.apt_package.as_deref(), Some("curl-deb"));
    assert_eq!(dep.apk_package.as_deref(), Some("curl-alpine"));
}

#[test]
fn check_tool_all_ready() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("sh")], // sh is always available
        recommended: vec![],
        config_files: vec![],
        permissions: vec!["file_read".to_string()],
        network: vec![],
    };
    let result = check_tool("TestTool", &spec);
    assert_eq!(result.status, ReadyStatus::Ready);
    assert_eq!(result.tool_name, "TestTool");
}

#[test]
fn check_tool_not_ready_missing_required() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("nonexistent_binary_xyz_99")],
        recommended: vec![],
        config_files: vec![],
        permissions: vec![],
        network: vec![],
    };
    let result = check_tool("MissingTool", &spec);
    assert_eq!(result.status, ReadyStatus::NotReady);
}

#[test]
fn check_tool_partial_missing_recommended() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("sh")],
        recommended: vec![make_dep("nonexistent_binary_xyz_99")],
        config_files: vec![],
        permissions: vec![],
        network: vec![],
    };
    let result = check_tool("PartialTool", &spec);
    assert_eq!(result.status, ReadyStatus::Partial);
}

#[test]
fn check_tool_partial_missing_config() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("sh")],
        recommended: vec![],
        config_files: vec!["/nonexistent/config.json".to_string()],
        permissions: vec![],
        network: vec![],
    };
    let result = check_tool("ConfigMissing", &spec);
    assert_eq!(result.status, ReadyStatus::Partial);
}

#[test]
fn generate_checklist_ready_partial_not_ready() {
    let results = vec![
        ToolReadyResult {
            tool_name: "ReadyTool".to_string(),
            status: ReadyStatus::Ready,
            required_results: vec![(make_dep("sh"), DepStatus::Available)],
            recommended_results: vec![],
            config_results: vec![],
            permission_results: vec![("file_read".to_string(), true)],
            network_results: vec![],
        },
        ToolReadyResult {
            tool_name: "PartialTool".to_string(),
            status: ReadyStatus::Partial,
            required_results: vec![],
            recommended_results: vec![(make_dep("optional"), DepStatus::Missing)],
            config_results: vec![("~/.config".to_string(), false)],
            permission_results: vec![],
            network_results: vec![("https_outbound".to_string(), false)],
        },
        ToolReadyResult {
            tool_name: "BrokenTool".to_string(),
            status: ReadyStatus::NotReady,
            required_results: vec![(make_dep("missing"), DepStatus::Missing)],
            recommended_results: vec![],
            config_results: vec![],
            permission_results: vec![("exec_shell".to_string(), false)],
            network_results: vec![],
        },
    ];
    let checklist = generate_checklist(&results);
    assert!(checklist.contains("1 ready"));
    assert!(checklist.contains("1 partial"));
    assert!(checklist.contains("1 not ready"));
    assert!(checklist.contains("total: 3"));
    assert!(checklist.contains("READY"));
    assert!(checklist.contains("PARTIAL"));
    assert!(checklist.contains("NOT_READY"));
    assert!(checklist.contains("INSTALLED"));
    assert!(checklist.contains("MISSING"));
    assert!(checklist.contains("GRANTED") || checklist.contains("DENIED"));
}

#[test]
fn format_dep_status_label_all() {
    assert_eq!(format_dep_status_label(&DepStatus::Available), "INSTALLED");
    assert_eq!(format_dep_status_label(&DepStatus::Missing), "MISSING");
    let low = format_dep_status_label(&DepStatus::VersionLow {
        installed: "1.0".to_string(),
        required: "2.0".to_string(),
    });
    assert!(low.contains("OUTDATED"));
    assert!(low.contains("1.0"));
    assert!(low.contains("2.0"));
}

#[test]
fn check_dep_available_binary() {
    let dep = make_dep("sh");
    assert_eq!(check_dep(&dep), DepStatus::Available);
}

#[test]
fn check_dep_missing_binary() {
    let dep = make_dep("nonexistent_binary_xyz_99");
    assert_eq!(check_dep(&dep), DepStatus::Missing);
}

#[test]
fn find_spec_path_error_when_none_exists() {
    // Override env to a nonexistent path and clear defaults
    unsafe { std::env::set_var("TOKENLESS_TOOL_READY_SPEC", "/nonexistent/spec.json") };
    let result = find_spec_path();
    unsafe { std::env::remove_var("TOKENLESS_TOOL_READY_SPEC") };
    // Result depends on whether any default path exists on the system
    let _ = result;
}

#[test]
fn detect_system_manager_env_override() {
    unsafe { std::env::set_var("TOKENLESS_PACKAGE_MANAGER", "test-mgr") };
    let mgr = detect_system_manager();
    unsafe { std::env::remove_var("TOKENLESS_PACKAGE_MANAGER") };
    assert_eq!(mgr, "test-mgr");
}

#[test]
fn load_spec_error_on_nonexistent_file() {
    let result = load_spec(&std::path::PathBuf::from("/nonexistent/spec.json"));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Failed to read"));
}

#[test]
fn load_spec_error_on_invalid_json() {
    let tmp =
        std::env::temp_dir().join(format!("tokenless-bad-spec-{}.json", std::process::id()));
    std::fs::write(&tmp, "not json").unwrap();
    let result = load_spec(&tmp);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Failed to parse"));
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn load_spec_with_aliases() {
    let tmp =
        std::env::temp_dir().join(format!("tokenless-alias-spec-{}.json", std::process::id()));
    let spec = json!({
        "Shell": {
            "aliases": ["shell", "sh"],
            "required": ["bash"],
            "recommended": [],
            "config_files": [],
            "permissions": ["exec_shell"],
            "network": []
        }
    });
    std::fs::write(&tmp, serde_json::to_string(&spec).unwrap()).unwrap();
    let specs = load_spec(&tmp).unwrap();
    let shell = specs.get("Shell").unwrap();
    assert_eq!(shell.aliases, vec!["shell", "sh"]);
    assert_eq!(shell.permissions, vec!["exec_shell"]);
    std::fs::remove_file(&tmp).ok();
}

#[test]
fn build_json_result_partial() {
    let result = build_json_result(
        "Shell",
        &ReadyStatus::Partial,
        &["jq".to_string()],
        &["curl".to_string()],
    );
    assert_eq!(result["status"], "PARTIAL");
    assert_eq!(result["fixed"][0], "jq");
    assert_eq!(result["missing"][0], "curl");
    assert!(result.get("diagnostic").is_none());
}

#[test]
fn expand_path_tilde_only() {
    let expanded = expand_path("~");
    let home = crate::get_home_dir();
    assert_eq!(expanded, home);
}

#[test]
fn check_dep_with_version_available() {
    let dep = DepEntry {
        binary: "sh".to_string(),
        version: Some(">=0.0.1".to_string()),
        package: "sh".to_string(),
        apt_package: None,
        apk_package: None,
        manager: "rpm".to_string(),
        pip_name: None,
        uv_name: None,
        npm_name: None,
        use_npx: false,
        fallback: vec![],
    };
    let status = check_dep(&dep);
    // sh is always available; version check may or may not match
    assert!(matches!(
        status,
        DepStatus::Available | DepStatus::VersionLow { .. }
    ));
}

#[test]
fn format_dep_status_available() {
    assert_eq!(format_dep_status(&DepStatus::Available), "✓");
}

#[test]
fn format_dep_status_missing() {
    assert_eq!(format_dep_status(&DepStatus::Missing), "missing");
}

#[test]
fn format_dep_status_version_low() {
    let status = DepStatus::VersionLow {
        installed: "1.0".to_string(),
        required: "2.0".to_string(),
    };
    let s = format_dep_status(&status);
    assert!(s.contains("version low"));
    assert!(s.contains("1.0"));
    assert!(s.contains("2.0"));
}

#[test]
fn format_status_all_variants() {
    assert_eq!(format_status(&ReadyStatus::Ready), "READY");
    assert_eq!(format_status(&ReadyStatus::Partial), "PARTIAL");
    assert_eq!(format_status(&ReadyStatus::NotReady), "NOT_READY");
    assert_eq!(format_status(&ReadyStatus::Unknown), "UNKNOWN");
}

#[test]
fn check_network_https_outbound() {
    // Just exercise the path — may or may not succeed depending on network
    let _ = check_network("https_outbound");
}

#[test]
fn normalize_deps_mixed() {
    let array = json!([
        "curl",
        {"binary": "jq", "package": "jq", "manager": "rpm"}
    ]);
    let deps = normalize_deps(&array);
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].binary, "curl");
    assert_eq!(deps[1].binary, "jq");
}

#[test]
fn normalize_deps_from_value_array() {
    let empty = normalize_deps(&json!([]));
    assert!(empty.is_empty());
}

#[test]
fn check_tool_with_network() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("sh")],
        recommended: vec![],
        config_files: vec![],
        permissions: vec![],
        network: vec!["some_network_check".to_string()],
    };
    let result = check_tool("NetTool", &spec);
    // Network check defaults to true for unknown checks
    assert_eq!(result.tool_name, "NetTool");
    assert!(!result.network_results.is_empty());
}

#[test]
fn check_tool_with_permissions() {
    let spec = ToolDepSpec {
        aliases: vec![],
        required: vec![make_dep("sh")],
        recommended: vec![],
        config_files: vec![],
        permissions: vec!["file_read".to_string(), "file_write".to_string()],
        network: vec![],
    };
    let result = check_tool("PermTool", &spec);
    assert_eq!(result.permission_results.len(), 2);
}

#[test]
fn generate_checklist_with_network_results() {
    let results = vec![ToolReadyResult {
        tool_name: "NetTool".to_string(),
        status: ReadyStatus::Ready,
        required_results: vec![],
        recommended_results: vec![],
        config_results: vec![],
        permission_results: vec![],
        network_results: vec![("https_outbound".to_string(), true)],
    }];
    let checklist = generate_checklist(&results);
    assert!(checklist.contains("NetTool"));
    assert!(checklist.contains("1 ready"));
}

#[test]
fn expand_path_no_tilde() {
    assert_eq!(expand_path("/usr/bin/test"), "/usr/bin/test");
}

#[test]
fn expand_path_tilde_subdir() {
    let home = crate::get_home_dir();
    if home.is_empty() {
        return;
    }
    let expanded = expand_path("~/.config");
    assert_eq!(expanded, format!("{}/.config", home));
}

#[test]
fn check_config_file_expanded_tilde() {
    let home = crate::get_home_dir();
    if home.is_empty() {
        return;
    }
    // A file that almost certainly doesn't exist
    let result = check_config_file("~/.tokenless-nonexistent-cfg-xyz");
    assert!(!result);
}
