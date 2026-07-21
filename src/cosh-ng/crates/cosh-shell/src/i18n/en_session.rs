//! English messages for interactive session recovery.

use super::MessageId;

pub(super) fn message(id: MessageId) -> &'static str {
    match id {
        MessageId::HelpGroupSessions => "Sessions",
        MessageId::HelpSummarySession => "discover, resume, and clear Agent sessions",
        MessageId::SessionTitle => "Agent sessions",
        MessageId::SessionUnavailableBody => "Session recovery requires the cosh-core backend.",
        MessageId::SessionBusyBody => {
            "Finish the active Agent run or open panel before managing sessions."
        }
        MessageId::SessionEmptyBody => "No persisted sessions exist for this workspace.",
        MessageId::SessionListFooter => {
            "Use /session to open the picker or /session resume <id>."
        }
        MessageId::SessionStatusTitle => "Session recovery status",
        MessageId::SessionShellIdLine => "shell session: {id}",
        MessageId::SessionProviderIdLine => {
            "active provider session: {active}\nselected provider session: {selected}"
        }
        MessageId::SessionWorkspaceLine => "workspace: {workspace}",
        MessageId::SessionRecoveryLine => "recovery state: {state}",
        MessageId::SessionErrorLine => "last recovery error: [{code}] {error}",
        MessageId::SessionEvidenceNotRestoredBody => {
            "Historical terminal evidence was not restored; only model conversation context resumes."
        }
        MessageId::SessionPickerFooter => {
            "Up/Down or j/k move · Enter resume · Space mark for clear · d clear · Esc cancel"
        }
        MessageId::SessionClearConfirmTitle => "Confirm session clear",
        MessageId::SessionClearConfirmCountLine => {
            "The following {count} persisted session(s) will be deleted:"
        }
        MessageId::SessionClearConfirmFooter => "Enter or y confirms · Esc, Ctrl+C, or n cancels",
        MessageId::SessionSelectedTitle => "Session selected",
        MessageId::SessionSelectedBody => {
            "Provider session {id} will resume on the next Agent request."
        }
        MessageId::SessionErrorTitle => "Session recovery",
        MessageId::SessionClearedTitle => "Sessions cleared",
        MessageId::SessionClearedBody => "Deleted {count} persisted session(s).",
        MessageId::SessionSkippedBody => "Skipped {count} protected or unavailable session(s).",
        MessageId::SessionClearInterruptedBody => {
            "Clear stopped [{code}]: {unknown} session(s) have unknown status; {unattempted} were not attempted."
        }
        MessageId::SessionCancelledTitle => "Session manager closed",
        MessageId::SessionCancelledBody => "No provider session or persisted file changed.",
        MessageId::SessionUsageBody => {
            "Usage: /session [status|list|resume <id>|clear <id>...|clear --all]"
        }
        MessageId::SessionNotReadyBody => {
            "Session {id} is {health} and cannot be resumed. It may still be cleared."
        }
        MessageId::SessionProtectedBody => {
            "Active or selected provider sessions are protected and were not cleared."
        }
        _ => super::en_approval::message(id),
    }
}
