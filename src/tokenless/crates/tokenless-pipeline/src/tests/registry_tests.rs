// Registry filtering contract: candidates must match content type and seam,
// and every capability a spec requires must be declared by the adapter.

const FULL: Capabilities = Capabilities {
    replace_output: true,
    publish_retrieve_tool: true,
    replace_with_text: false,
};
const NONE: Capabilities = Capabilities {
    replace_output: false,
    publish_retrieve_tool: false,
    replace_with_text: false,
};

const TEST_REGISTRY: &[CompressorSpec] = &[
    CompressorSpec {
        id: "response-cleanup",
        content_types: &[ContentType::JsonRecords],
        seams: &[Seam::PostTool],
        required_capabilities: Capabilities {
            replace_output: true,
            publish_retrieve_tool: false,
            replace_with_text: false,
        },
        stage: Stage::Lossless,
        cost_class: CostClass::Cheap,
    },
    CompressorSpec {
        id: "build-log",
        content_types: &[ContentType::BuildLog],
        seams: &[Seam::PostTool],
        required_capabilities: FULL,
        stage: Stage::RetrievableLossy,
        cost_class: CostClass::Moderate,
    },
    CompressorSpec {
        id: "schema",
        content_types: &[ContentType::JsonRecords],
        seams: &[Seam::BeforeModel],
        required_capabilities: NONE,
        stage: Stage::Lossless,
        cost_class: CostClass::Cheap,
    },
    CompressorSpec {
        id: "json-truncate",
        content_types: &[ContentType::JsonRecords],
        seams: &[Seam::PostTool],
        required_capabilities: Capabilities {
            replace_output: true,
            publish_retrieve_tool: false,
            replace_with_text: false,
        },
        stage: Stage::Truncation,
        cost_class: CostClass::Cheap,
    },
];

fn ids(
    content_type: ContentType,
    seam: Seam,
    capabilities: Capabilities,
) -> Vec<&'static str> {
    candidates(TEST_REGISTRY, content_type, seam, capabilities)
        .map(|spec| spec.id)
        .collect()
}

#[test]
fn filters_by_content_type_and_seam() {
    assert_eq!(
        ids(ContentType::JsonRecords, Seam::PostTool, FULL),
        ["response-cleanup", "json-truncate"]
    );
    assert_eq!(ids(ContentType::JsonRecords, Seam::BeforeModel, FULL), ["schema"]);
    assert_eq!(ids(ContentType::BuildLog, Seam::PostTool, FULL), ["build-log"]);
    assert!(ids(ContentType::Diff, Seam::PostTool, FULL).is_empty());
}

#[test]
fn required_capabilities_must_be_declared() {
    // No replace_output: response-shaping candidates disappear entirely.
    assert!(ids(ContentType::JsonRecords, Seam::PostTool, NONE).is_empty());
    // replace_output alone is not enough for a retrievable-lossy compressor
    // that needs a reachable retrieve tool.
    let replace_only = Capabilities {
        replace_output: true,
        publish_retrieve_tool: false,
        replace_with_text: false,
    };
    assert!(ids(ContentType::BuildLog, Seam::PostTool, replace_only).is_empty());
    // A spec requiring nothing is a candidate for a capability-less adapter.
    assert_eq!(ids(ContentType::JsonRecords, Seam::BeforeModel, NONE), ["schema"]);
}

#[test]
fn registration_order_is_preserved() {
    let order = ids(ContentType::JsonRecords, Seam::PostTool, FULL);
    assert_eq!(order, ["response-cleanup", "json-truncate"]);
}

#[test]
fn stage_and_cost_orderings_match_the_escalation_ladder() {
    let mut stages = [Stage::Truncation, Stage::Lossless, Stage::RetrievableLossy];
    stages.sort_unstable();
    assert_eq!(
        stages,
        [Stage::Lossless, Stage::RetrievableLossy, Stage::Truncation]
    );
    let mut costs = [CostClass::Expensive, CostClass::Cheap, CostClass::Moderate];
    costs.sort_unstable();
    assert_eq!(
        costs,
        [CostClass::Cheap, CostClass::Moderate, CostClass::Expensive]
    );
}

#[test]
fn production_registry_lists_exactly_the_implemented_compressors() {
    // Entries land together with the compressor that implements them, never
    // speculatively. The response cleanup (roadmap §5.3) is the first one.
    assert_eq!(
        REGISTRY.iter().map(|spec| spec.id).collect::<Vec<_>>(),
        ["response-cleanup"]
    );
    assert_eq!(RESPONSE_CLEANUP.stage, Stage::RetrievableLossy);
    // Routing requires only output replacement: without a stash the cleanup
    // degrades (and arbitration enforces required reversibility), so a
    // missing retrieve tool must not filter it out.
    assert!(RESPONSE_CLEANUP.matches(
        ContentType::JsonRecords,
        Seam::PostTool,
        Capabilities {
            replace_output: true,
            publish_retrieve_tool: false,
            replace_with_text: false,
        },
    ));
    assert!(!RESPONSE_CLEANUP.matches(ContentType::JsonRecords, Seam::PostTool, NONE));
}
