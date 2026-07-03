use anolisa_core::state::{InstalledState, ObjectStatus, Ownership};
use anolisa_platform::pkg_query::{PackageInfo, PackageQuery, PackageQueryError};

use super::support::{
    FakeRpmQuery, component_object, pkg_info, projection_for, projection_for_index,
    rpm_component_object, sample_index_with_aliases, state_with_component_object,
};

#[test]
fn local_projection_labels_cover_list_states() {
    let empty = InstalledState::default();
    let absent_query = FakeRpmQuery::default();
    let observed_query = FakeRpmQuery {
        installed: vec![(
            "agentsight".to_string(),
            pkg_info("agentsight", "1.2.3", Some("1.al8"), "x86_64"),
        )],
        command_missing: false,
        what_provides: Vec::new(),
    };

    let observed = projection_for("agentsight", &empty, &observed_query);
    assert_eq!(observed.local_state_label(), "observed");

    let tracked = projection_for(
        "agentsight",
        &state_with_component_object(rpm_component_object(
            "agentsight",
            ObjectStatus::Adopted,
            Ownership::RpmObserved,
            "agentsight",
            "1.2.3-1.al8",
        )),
        &observed_query,
    );
    assert_eq!(tracked.local_state_label(), "tracked");

    let installed = projection_for(
        "tokenless",
        &state_with_component_object(component_object(
            "tokenless",
            ObjectStatus::Installed,
            Ownership::RawManaged,
        )),
        &absent_query,
    );
    assert_eq!(installed.local_state_label(), "installed");

    let drifted = projection_for(
        "agentsight",
        &state_with_component_object(rpm_component_object(
            "agentsight",
            ObjectStatus::Installed,
            Ownership::RpmManaged,
            "agentsight",
            "1.2.3-1.al8",
        )),
        &FakeRpmQuery {
            installed: vec![(
                "agentsight".to_string(),
                pkg_info("agentsight", "2.0.0", Some("1.al8"), "x86_64"),
            )],
            command_missing: false,
            what_provides: Vec::new(),
        },
    );
    assert_eq!(drifted.local_state_label(), "drifted");

    let missing = projection_for(
        "agentsight",
        &state_with_component_object(rpm_component_object(
            "agentsight",
            ObjectStatus::Installed,
            Ownership::RpmManaged,
            "agentsight",
            "1.2.3-1.al8",
        )),
        &absent_query,
    );
    assert_eq!(missing.local_state_label(), "missing");

    let failed = projection_for(
        "tokenless",
        &state_with_component_object(component_object(
            "tokenless",
            ObjectStatus::Failed,
            Ownership::RawManaged,
        )),
        &absent_query,
    );
    assert_eq!(failed.local_state_label(), "failed");

    let degraded = projection_for(
        "tokenless",
        &state_with_component_object(component_object(
            "tokenless",
            ObjectStatus::Partial,
            Ownership::RawManaged,
        )),
        &absent_query,
    );
    assert_eq!(degraded.local_state_label(), "degraded");

    let disabled = projection_for(
        "tokenless",
        &state_with_component_object(component_object(
            "tokenless",
            ObjectStatus::Disabled,
            Ownership::RawManaged,
        )),
        &absent_query,
    );
    assert_eq!(disabled.local_state_label(), "disabled");

    let not_installed = projection_for("tokenless", &empty, &absent_query);
    assert_eq!(not_installed.local_state_label(), "not_installed");
}

#[test]
fn untracked_observed_rpm_projection_uses_rpm_backend_rpm_ownership_and_install_action() {
    let query = FakeRpmQuery {
        installed: vec![(
            "agentsight".to_string(),
            pkg_info("agentsight", "1.2.3", Some("1.al8"), "x86_64"),
        )],
        command_missing: false,
        what_provides: Vec::new(),
    };

    let projection = projection_for("agentsight", &InstalledState::default(), &query);

    assert_eq!(projection.local_state_label(), "observed");
    assert_eq!(projection.backend.as_deref(), Some("rpm"));
    assert_eq!(projection.ownership_label(), "rpm");
    assert_eq!(projection.action_label(), "install");
    assert_eq!(projection.rpm_package.as_deref(), Some("agentsight"));
    assert_eq!(projection.status, "not_installed");
}

#[test]
fn untracked_observed_rpm_projection_fetches_source_repo_when_installed_info_has_no_origin() {
    struct OriginQuery;

    impl PackageQuery for OriginQuery {
        fn query_installed(&self, package: &str) -> Result<Option<PackageInfo>, PackageQueryError> {
            assert_eq!(package, "agentsight");
            let mut info = pkg_info("agentsight", "1.2.3", Some("1.al8"), "x86_64");
            info.origin = None;
            Ok(Some(info))
        }

        fn query_available(&self, _package: &str) -> Result<Vec<PackageInfo>, PackageQueryError> {
            Ok(Vec::new())
        }

        fn installed_origin(&self, package: &str) -> Result<Option<String>, PackageQueryError> {
            assert_eq!(package, "agentsight");
            Ok(Some("alinux3-plus".to_string()))
        }
    }

    let projection = projection_for("agentsight", &InstalledState::default(), &OriginQuery);

    assert_eq!(projection.local_state_label(), "observed");
    assert_eq!(projection.rpm_source_repo.as_deref(), Some("alinux3-plus"));
}

#[test]
fn rpm_query_command_missing_keeps_state_index_projection() {
    let query = FakeRpmQuery {
        installed: Vec::new(),
        command_missing: true,
        what_provides: Vec::new(),
    };

    let projection = projection_for("agentsight", &InstalledState::default(), &query);

    assert_eq!(projection.local_state_label(), "not_installed");
    assert_eq!(projection.backend, None);
    assert_eq!(projection.ownership_label(), "none");
    assert_eq!(projection.action_label(), "install");
    assert_eq!(projection.status, "not_installed");
}

#[test]
fn observed_rpm_found_via_alias_when_backend_package_not_installed() {
    let index = sample_index_with_aliases();
    // Backend package "copilot-shell" is not installed; alias "cosh-old" is.
    let query = FakeRpmQuery {
        installed: vec![(
            "cosh-old".to_string(),
            pkg_info("cosh-old", "1.0.0", Some("1.al8"), "x86_64"),
        )],
        command_missing: false,
        what_provides: Vec::new(),
    };

    let projection = projection_for_index(&index, "cosh", &InstalledState::default(), &query);

    assert_eq!(projection.local_state_label(), "observed");
    assert_eq!(projection.ownership_label(), "rpm");
    assert_eq!(projection.action_label(), "install");
    assert_eq!(projection.rpm_package.as_deref(), Some("cosh-old"));
}

#[test]
fn observed_rpm_found_via_provides_when_no_direct_match() {
    let index = sample_index_with_aliases();
    // Neither backend package nor alias is installed, but a package providing
    // `anolisa-component(cosh)` exists in rpmdb.
    let query = FakeRpmQuery {
        installed: vec![(
            "cosh-legacy".to_string(),
            pkg_info("cosh-legacy", "0.9.0", Some("1.al8"), "x86_64"),
        )],
        command_missing: false,
        what_provides: vec![(
            "anolisa-component(cosh)".to_string(),
            vec!["cosh-legacy".to_string()],
        )],
    };

    let projection = projection_for_index(&index, "cosh", &InstalledState::default(), &query);

    assert_eq!(projection.local_state_label(), "observed");
    assert_eq!(projection.ownership_label(), "rpm");
    assert_eq!(projection.action_label(), "install");
    assert_eq!(projection.rpm_package.as_deref(), Some("cosh-legacy"));
}

#[test]
fn observed_rpm_provides_with_ambiguous_providers_is_not_observed() {
    let index = sample_index_with_aliases();
    // Two packages provide the same capability — list must not pick one.
    let query = FakeRpmQuery {
        installed: vec![
            (
                "cosh-legacy".to_string(),
                pkg_info("cosh-legacy", "0.9.0", Some("1.al8"), "x86_64"),
            ),
            (
                "cosh-vendor".to_string(),
                pkg_info("cosh-vendor", "1.0.0", Some("1.al8"), "x86_64"),
            ),
        ],
        command_missing: false,
        what_provides: vec![(
            "anolisa-component(cosh)".to_string(),
            vec!["cosh-legacy".to_string(), "cosh-vendor".to_string()],
        )],
    };

    let projection = projection_for_index(&index, "cosh", &InstalledState::default(), &query);

    assert_eq!(projection.local_state_label(), "not_installed");
    assert_eq!(projection.ownership_label(), "none");
}
