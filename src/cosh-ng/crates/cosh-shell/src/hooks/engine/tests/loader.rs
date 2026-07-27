use super::*;

#[test]
fn load_hooks_from_dir_skips_non_executable() {
    let dir = std::env::temp_dir().join("cosh_hook_test_noexec");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::create_dir_all(&dir);

    // Non-executable file
    let path = dir.join("no-exec.sh");
    fs::write(&path, "#!/bin/bash\n# cosh-hook: no-exec\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }

    // Executable file
    let path2 = dir.join("exec.sh");
    fs::write(&path2, "#!/bin/bash\n# cosh-hook: exec-hook\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path2, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut engine = HookEngine::new();
    engine.load_hooks_from_dir(&dir);

    assert_eq!(engine.external_hooks().len(), 1);
    assert_eq!(engine.external_hooks()[0].matcher.id, "exec-hook");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_project_hooks_missing_dir_is_noop() {
    let project = std::env::temp_dir().join("cosh_hook_test_project_missing_dir");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).unwrap();

    let mut engine = HookEngine::new();
    engine.load_project_hooks_from_root(&project, false);

    assert!(engine.external_hooks().is_empty());
    assert!(engine.registered_hook_infos().is_empty());

    let _ = fs::remove_dir_all(&project);
}
