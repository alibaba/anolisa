macro_rules! startup_ids {
    ($next:ident, $remaining:tt, $($ids:ident,)*) => {
        $next!(
            $remaining,
            $($ids,)*
            StartupTitle,
            StartupAdapterLine,
            StartupCwdLine,
            StartupCommandsLine,
            StartupHooksNoneSummary,
            StartupHooksCompletedSummary,
            StartupHooksFindingsHeading,
            StartupHooksRustProjectFinding,
            StartupHooksNoFindings,
            StartupHooksReadOnlyNote,
            StartupSwitchHint,
        );
    };
}
