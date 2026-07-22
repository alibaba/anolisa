macro_rules! session_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            SessionTitle,
            SessionUnavailableBody,
            SessionBusyBody,
            SessionEmptyBody,
            SessionListFooter,
            SessionStatusTitle,
            SessionShellIdLine,
            SessionProviderIdLine,
            SessionWorkspaceLine,
            SessionRecoveryLine,
            SessionErrorLine,
            SessionEvidenceNotRestoredBody,
            SessionPickerFooter,
            SessionClearConfirmTitle,
            SessionClearConfirmCountLine,
            SessionClearConfirmFooter,
            SessionSelectedTitle,
            SessionSelectedBody,
            SessionErrorTitle,
            SessionClearedTitle,
            SessionClearedBody,
            SessionSkippedBody,
            SessionClearInterruptedBody,
            SessionCancelledTitle,
            SessionCancelledBody,
            SessionUsageBody,
            SessionNotReadyBody,
            SessionProtectedBody,
        );
    };
}

// Keep new compaction IDs in a trailing segment so existing MessageId
// discriminants remain stable across the upstream i18n module split.
macro_rules! session_compaction_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            SessionCompactTitle,
            SessionCompactStartedBody,
            SessionCompactFooter,
            SessionCompactStatusTitle,
            SessionCompactStatusSessionLine,
            SessionCompactStatusRunningLine,
            SessionCompactStatusIdleBody,
            SessionCompactStatusRecommendedBody,
            SessionCompactStatusPendingRenderBody,
            SessionCompactNoSessionBody,
            SessionCompactDuplicateBody,
            SessionCompactNotRunningBody,
            SessionCompactCancelRequestedBody,
            SessionCompactCompletedTitle,
            SessionCompactCompletedBody,
            SessionCompactCompletedRetainedBody,
            SessionCompactFailedTitle,
            SessionCompactFailedBody,
            SessionCompactCancelledTitle,
            SessionCompactCancelledBody,
            SessionCompactAgentPausedTitle,
            SessionCompactAgentPausedBody,
            SessionCompactAutoStartedBody,
            SessionCompactSpawnFailedBody,
            SessionCompactFailedTranscriptBody,
            SessionCompactQueueFullBody,
        );
    };
}
