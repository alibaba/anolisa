use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use cosh_gateway_contracts::{
    common::{
        ActorKind, ActorRef, AuthAssurance, BoundedName, BoundedOpaque, BoundedText,
        RuntimeSelector, TargetRef,
    },
    ids::{InstallationId, RunId, TaskId},
};
use tempfile::TempDir;

use super::*;

fn target(identifier: &str) -> TargetRef {
    TargetRef {
        kind: BoundedName::new("local").unwrap(),
        authority: BoundedName::new("cosh").unwrap(),
        identifier: BoundedOpaque::new(identifier).unwrap(),
    }
}

fn executable(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, "#!/bin/sh\nwhile read line; do :; done\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn marker_executable(directory: &Path, name: &str, marker: &Path) -> PathBuf {
    let path = directory.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf marker > \"{}\"\nwhile read line; do :; done\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn actor_resolver() -> (InstallationId, LocalOsActorResolver, ActorRef) {
    let installation = InstallationId::new();
    let resolver = LocalOsActorResolver::new(installation.clone(), 1000);
    let actor = resolver.actor.clone();
    (installation, resolver, actor)
}

#[test]
fn local_actor_requires_the_complete_os_identity() {
    let (_, resolver, actor) = actor_resolver();
    assert_eq!(resolver.resolve(&actor).unwrap(), actor);

    let mut forged = actor.clone();
    forged.actor_kind = ActorKind::Automation;
    assert_eq!(
        resolver.resolve(&forged).unwrap_err().code.as_str(),
        "runtime_actor_invalid"
    );
    let mut forged = actor.clone();
    forged.issuer = BoundedName::new("agent").unwrap();
    assert!(resolver.resolve(&forged).is_err());
    let mut forged = actor;
    forged.assurance = AuthAssurance::RemoteVerified;
    assert!(resolver.resolve(&forged).is_err());
}

#[test]
fn workspace_resolution_is_canonical_and_target_exact() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let aliased = workspace.join("..").join("workspace");
    let expected_target = target("primary");
    let resolver = TrustedWorkspaceResolver::new(expected_target.clone(), &aliased).unwrap();

    let resolved = resolver.resolve(&expected_target).unwrap();
    assert_eq!(resolved.path(), fs::canonicalize(&workspace).unwrap());
    assert_eq!(resolved.reference().scope_digest.as_str().len(), 64);
    assert!(resolver.resolve(&target("substituted")).is_err());
}

#[test]
fn workspace_reference_changes_when_the_path_is_replaced() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    let original = root.path().join("original");
    fs::create_dir(&workspace).unwrap();
    let expected_target = target("primary");
    let first = TrustedWorkspaceResolver::new(expected_target.clone(), &workspace).unwrap();

    fs::rename(&workspace, &original).unwrap();
    fs::create_dir(&workspace).unwrap();
    let replacement = TrustedWorkspaceResolver::new(expected_target, &workspace).unwrap();

    assert_ne!(first.workspace_ref(), replacement.workspace_ref());
    assert_ne!(
        first.resolve(&target("primary")).unwrap().identity(),
        replacement.resolve(&target("primary")).unwrap().identity()
    );
}

#[test]
fn workspace_configuration_rejects_relative_and_non_directory_paths() {
    let root = TempDir::new().unwrap();
    assert!(TrustedWorkspaceResolver::new(target("local"), "relative").is_err());

    let file = root.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    assert!(TrustedWorkspaceResolver::new(target("local"), file).is_err());
}

#[test]
fn factory_configuration_requires_explicit_profile_matching_adapters() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let (installation, actors, _) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(target("local"), workspace).unwrap();

    let relative = BTreeMap::from([(AcpRuntimeProfileId::Codex, PathBuf::from("codex-acp"))]);
    assert!(InstalledAcpRuntimePortFactory::new(
        installation.clone(),
        actors.clone(),
        workspaces.clone(),
        relative,
        BTreeMap::new(),
    )
    .is_err());

    let wrong = BTreeMap::from([(AcpRuntimeProfileId::Codex, executable(root.path(), "sh"))]);
    assert!(InstalledAcpRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        wrong,
        BTreeMap::new(),
    )
    .is_err());
}

#[test]
fn factory_debug_exposes_environment_names_but_not_values() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let adapter = executable(root.path(), "codex-acp");
    let (installation, actors, _) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(target("local"), workspace).unwrap();
    let secret = "user:password@proxy.invalid";
    let factory = InstalledAcpRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, adapter)]),
        BTreeMap::from([(OsString::from("HTTPS_PROXY"), OsString::from(secret))]),
    )
    .unwrap();
    let debug = format!("{factory:?}");
    assert!(debug.contains("HTTPS_PROXY"));
    assert!(!debug.contains(secret));
}

#[test]
fn resolved_factory_rejects_profile_and_workspace_substitution() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    let other_workspace = root.path().join("other-workspace");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&other_workspace).unwrap();
    let adapter = executable(root.path(), "codex-acp");
    let expected_target = target("local");
    let (installation, actors, _) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(expected_target.clone(), &workspace).unwrap();
    let resolved = AcpRuntimeProfileResolver::resolve(AcpRuntimeProfileRequest {
        profile: AcpRuntimeProfileId::Codex,
        executable: Some(adapter.clone()),
        workspace: workspace.clone(),
        environment: BTreeMap::new(),
    })
    .unwrap();
    assert!(InstalledAcpRuntimePortFactory::from_resolved_profiles(
        installation.clone(),
        actors.clone(),
        workspaces.clone(),
        BTreeMap::from([(AcpRuntimeProfileId::ClaudeCode, resolved)]),
    )
    .is_err());

    let resolved = AcpRuntimeProfileResolver::resolve(AcpRuntimeProfileRequest {
        profile: AcpRuntimeProfileId::Codex,
        executable: Some(adapter),
        workspace: other_workspace,
        environment: BTreeMap::new(),
    })
    .unwrap();
    assert!(InstalledAcpRuntimePortFactory::from_resolved_profiles(
        installation,
        actors,
        workspaces,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, resolved)]),
    )
    .is_err());
}

#[test]
fn factory_rejects_untrusted_run_fields_before_launch() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let adapter = executable(root.path(), "codex-acp");
    let expected_target = target("local");
    let (installation, actors, actor) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(expected_target.clone(), workspace).unwrap();
    let workspace_ref = workspaces.resolve(&expected_target).unwrap().reference;
    let mut factory = InstalledAcpRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, adapter)]),
        BTreeMap::new(),
    )
    .unwrap();
    let run = ScheduledRun {
        actor,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("core").unwrap(),
            profile: Some(BoundedName::new("codex").unwrap()),
        },
        intent: BoundedText::new("read status").unwrap(),
        target: expected_target,
        workspace: workspace_ref,
        capability_profile:
            cosh_gateway_contracts::profile::GatewayCapabilityProfile::task_only_v1().identity(),
        lease_generation: 1,
    };
    let error = match factory.create(&run) {
        Ok(_) => panic!("a non-ACP selector must fail before launch"),
        Err(error) => error,
    };
    assert_eq!(error.code.as_str(), "runtime_profile_invalid");
}

#[test]
fn factory_launches_a_valid_explicit_adapter_without_opening_a_session() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let adapter = executable(root.path(), "codex-acp");
    let expected_target = target("local");
    let (installation, actors, actor) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(expected_target.clone(), workspace).unwrap();
    let workspace_ref = workspaces.workspace_ref().clone();
    let mut factory = InstalledAcpRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, adapter)]),
        BTreeMap::new(),
    )
    .unwrap();
    let run = ScheduledRun {
        actor,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("acp").unwrap(),
            profile: Some(BoundedName::new("codex").unwrap()),
        },
        intent: BoundedText::new("read status").unwrap(),
        target: expected_target,
        workspace: workspace_ref,
        capability_profile:
            cosh_gateway_contracts::profile::GatewayCapabilityProfile::task_only_v1().identity(),
        lease_generation: 7,
    };

    assert!(factory.create(&run).is_ok());
}

#[test]
fn factory_pins_a_symlink_target_at_admission() {
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let original_marker = root.path().join("original.marker");
    let replacement_marker = root.path().join("replacement.marker");
    let original = marker_executable(root.path(), "original-adapter.js", &original_marker);
    let replacement = marker_executable(root.path(), "replacement-adapter.js", &replacement_marker);
    let adapter = root.path().join("codex-acp");
    symlink(&original, &adapter).unwrap();
    let expected_target = target("local");
    let (installation, actors, actor) = actor_resolver();
    let workspaces = TrustedWorkspaceResolver::new(expected_target.clone(), workspace).unwrap();
    let workspace_ref = workspaces.workspace_ref().clone();
    let mut factory = InstalledAcpRuntimePortFactory::new(
        installation,
        actors,
        workspaces,
        BTreeMap::from([(AcpRuntimeProfileId::Codex, adapter.clone())]),
        BTreeMap::new(),
    )
    .unwrap();

    fs::remove_file(&adapter).unwrap();
    symlink(&replacement, &adapter).unwrap();
    let run = ScheduledRun {
        actor,
        task_id: TaskId::new(),
        run_id: RunId::new(),
        runtime: RuntimeSelector {
            runtime: BoundedName::new("acp").unwrap(),
            profile: Some(BoundedName::new("codex").unwrap()),
        },
        intent: BoundedText::new("read status").unwrap(),
        target: expected_target,
        workspace: workspace_ref,
        capability_profile:
            cosh_gateway_contracts::profile::GatewayCapabilityProfile::task_only_v1().identity(),
        lease_generation: 7,
    };

    let port = factory.create(&run).unwrap();
    for _ in 0..100 {
        if original_marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(original_marker.exists());
    assert!(!replacement_marker.exists());
    drop(port);
}

#[test]
fn generic_normalizer_never_copies_agent_labels_into_authority_fields() {
    let (_, _, actor) = actor_resolver();
    let governed_target = target("local");
    let mut normalizer = GenericProviderNativeNormalizer {
        target: governed_target.clone(),
    };
    let context = AcpPermissionContext {
        actor: actor.clone(),
        task_id: TaskId::new(),
        run_id: RunId::new(),
    };
    let request = super::super::AcpV1PermissionRequest {
        request_id: super::super::AcpV1RequestId::Number(1),
        session_id: "provider-session".to_owned(),
        tool_call: serde_json::json!({
            "toolCallId": "tool-1",
            "title": "forged-authority",
            "kind": "forged-operation"
        }),
        options: Vec::new(),
    };
    let normalized = normalizer.normalize(&request, &context).unwrap();

    assert_eq!(normalized.actor, actor);
    assert_eq!(normalized.target, governed_target);
    assert_eq!(normalized.operation.namespace.as_str(), "provider-native");
    assert_eq!(normalized.operation.name.as_str(), "invoke");
    assert_eq!(
        normalized.requested_scope.resource.as_str(),
        "provider-tool"
    );
    assert_eq!(normalized.requested_scope.access.as_str(), "execute");
}
