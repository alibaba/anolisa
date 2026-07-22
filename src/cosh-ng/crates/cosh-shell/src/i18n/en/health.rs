use super::MessageId;

pub(super) fn message(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::HealthBannerTitle => "Health check",
        MessageId::HealthStartupLabel => "Health",
        MessageId::HealthBannerTryLabel => "Prompt",
        MessageId::HealthBannerFindingLabel => "Finding",
        MessageId::HealthBannerEvidenceLabel => "Evidence",
        MessageId::HealthBannerMoreFindingsLabel => "{count} more finding(s)",
        MessageId::HealthBannerUnavailableLabel => "Unavailable",
        MessageId::HealthBannerFindingsSection => "Findings",
        MessageId::HealthBannerSuggestedPromptSection => "Suggested Prompts",
        MessageId::HealthBannerSuggestedPromptIntro => "You can type these prompts to the agent:",
        MessageId::HealthSeverityOk => "ok",
        MessageId::HealthSeverityWarning => "warning",
        MessageId::HealthSeverityCritical => "critical",
        MessageId::HealthSeverityDegraded => "degraded",
        MessageId::HealthSeverityUnavailable => "unavailable",
        MessageId::HealthMetricCpu => "CPU",
        MessageId::HealthMetricCpuLoadPerCore => "Load 1m",
        MessageId::HealthMetricLoad1mShort => "1m",
        MessageId::HealthMetricLoad5mShort => "5m",
        MessageId::HealthMetricLoadValue => "{load} / {cores} cores ({ratio}x)",
        MessageId::HealthMetricCpuPerCoreUnit => "x",
        MessageId::HealthMetricCpuUsed => "CPU used",
        MessageId::HealthMetricHost => "Host",
        MessageId::HealthMetricMemory => "Memory",
        MessageId::HealthMetricMemoryAvailable => "Mem avail",
        MessageId::HealthMetricMemoryUsed => "Mem used",
        MessageId::HealthMetricSwap => "Swap",
        MessageId::HealthMetricSwapUsed => "Swap used",
        MessageId::HealthMetricDisk => "Disk",
        MessageId::HealthMetricDiskUsed => "Disk used",
        MessageId::HealthMetricDiskMountUsed => "Disk {mount} used",
        MessageId::HealthMetricPressure => "Load",
        MessageId::HealthMetricLevels => "Resources",
        MessageId::HealthMetricSignal => "Signal",
        MessageId::HealthMetricService => "Service",
        MessageId::HealthEvidenceDiskAvailable => "{gib} GiB available",
        MessageId::HealthEvidenceMount => "mount {mount}",
        MessageId::HealthEvidenceOomKilledProcess => "killed {process}",
        MessageId::HealthEvidenceOomCgroup => "cgroup {cgroup}",
        MessageId::HealthEvidenceOomOneHourCount => "1h OOM {count}",
        MessageId::HealthEvidenceOomTwentyFourHourCount => "24h OOM {count}",
        MessageId::HealthEvidenceOomVictimKilledAgo => "{subject} killed {age} ago",
        MessageId::HealthEvidenceOomVictimKilled => "{subject} killed",
        MessageId::HealthEvidenceOomAge => "OOM {age} ago",
        MessageId::HealthOomScopeMemcg => "cgroup memory limit",
        MessageId::HealthOomScopeHost => "host memory pressure",
        MessageId::HealthOomScopeCpuset => "cpuset/NUMA memory pressure",
        MessageId::HealthOomScopeMemoryPolicy => "memory policy pressure",
        MessageId::HealthOomScopeUnknown => "unknown OOM scope",
        MessageId::HealthTryAnalyzeMemoryPressure => "analyze memory pressure",
        MessageId::HealthTryCheckSwapPressure => "check active swap pressure",
        MessageId::HealthTryCheckRecentOom => "analyze latest OOM cause",
        MessageId::HealthTryInspectDiskUsage => "inspect disk usage",
        MessageId::HealthTryInspectServiceStatus => "inspect service status",
        MessageId::HealthTryInspectHighLoad => "inspect high load",
        MessageId::HealthTryInspectProcessMemory => "inspect {process} memory",
        MessageId::HealthTryReviewUnavailableChecks => "review unavailable checks",
        MessageId::HealthPromptAnalyzeMemoryPressure => {
            "Analyze memory pressure and identify top consumers that may affect this shell."
        }
        MessageId::HealthPromptCheckSwapPressure => {
            "Check whether swap pressure is active and which processes are driving it."
        }
        MessageId::HealthPromptCheckRecentOom => {
            "Help me analyze the cause of the most recent OOM, focusing on the killed process, cgroup, and memory state around the event."
        }
        MessageId::HealthPromptInspectDiskUsage => {
            "Inspect the risky mount and suggest safe disk cleanup targets."
        }
        MessageId::HealthPromptInspectServiceStatus => {
            "Inspect the configured service state and likely failure cause."
        }
        MessageId::HealthPromptInspectHighLoad => {
            "Analyze high load and identify CPU or IO pressure sources."
        }
        MessageId::HealthPromptInspectProcessMemory => {
            "Help me analyze why the latest OOM killed {process}, focusing on cgroup scope and memory limits."
        }
        MessageId::HealthPromptReviewUnavailableChecks => {
            "Explain why startup health checks were unavailable and how to restore them."
        }
        MessageId::HealthInsightMemoryAvailableLow => {
            "available memory is low; this can slow commands or make new processes fail"
        }
        MessageId::HealthInsightDiskHigh => {
            "disk usage is high on the riskiest mount; writes or builds may fail soon"
        }
        MessageId::HealthInsightRecentOom => {
            "the latest OOM has already happened; review the killed process, cgroup, and memory state around the event"
        }
        MessageId::HealthInsightCpuLoadHigh => {
            "load stayed high across recent windows; commands may be delayed"
        }
        MessageId::HealthInsightSwapPressure => {
            "swap usage is high with memory pressure context; paging can slow command response"
        }
        MessageId::HealthInsightServiceState => {
            "service unit {service} observed {observed}, expected {expected}"
        }
        MessageId::HealthInsightGeneric => "startup health found a signal worth checking",
        MessageId::HealthUnavailablePermissionDenied => "permission denied",
        MessageId::HealthUnavailableCommandMissing => "command missing",
        MessageId::HealthUnavailableTimeout => "timed out",
        MessageId::HealthUnavailableUnsupported => "unsupported",
        MessageId::HealthUnavailableParseError => "parse error",
        MessageId::HealthFindingPlatformUnsupported => "platform unsupported",
        MessageId::HealthFindingCoreCollectorUnavailable => "core check unavailable",
        MessageId::HealthFindingCpuLoadHigh => "system load high",
        MessageId::HealthFindingMemoryAvailableLow => "available memory low",
        MessageId::HealthFindingSwapPressure => "swap pressure with context",
        MessageId::HealthFindingDiskHigh => "disk usage high",
        MessageId::HealthFindingRecentOom => "recent OOM signal",
        MessageId::HealthFindingKernelPanic => "recent kernel panic",
        MessageId::HealthFindingServiceFailed => "service failed",
        MessageId::HealthFindingServiceInactive => "service inactive",
        MessageId::HealthTryReasonMemoryLow => "available memory is low",
        MessageId::HealthTryReasonSwapWithContext => "swap is high with pressure context",
        MessageId::HealthTryReasonRecentOom => "recent OOM is worth reviewing",
        MessageId::HealthTryReasonDiskHigh => "disk space is constrained",
        MessageId::HealthTryReasonServiceState => "configured service state is unexpected",
        MessageId::HealthTryReasonHighLoad => "load is elevated across recent windows",
        MessageId::HealthTryReasonMissingCoreCheck => "a core health check is unavailable",
        _ => return None,
    })
}
