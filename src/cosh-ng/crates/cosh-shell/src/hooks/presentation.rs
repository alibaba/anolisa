use std::io::Write;

use cosh_shell::agent_render::{ConsultationCardModel, NoticePanelModel, RatatuiInlineRenderer};

use super::policy::{classify_command_intent, CommandIntent};
use super::runtime::severity_label;
use crate::runtime::state::{InlineState, PendingConsultation};

pub(crate) fn render_consultation_card<W: Write>(
    consultation: &PendingConsultation,
    language: cosh_shell::Language,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(finding) = consultation.hook_finding.as_ref() else {
        return Ok(());
    };
    RatatuiInlineRenderer::for_terminal()
        .with_language(language)
        .write_consultation_card(
            output,
            &ConsultationCardModel {
                hook_id: finding.hook_id.clone(),
                details_id: consultation.finding_id.clone(),
                severity: severity_label(finding.severity).to_string(),
                confidence: consultation.confidence.clone(),
                display_reason: consultation.display_reason.clone(),
                title: finding.title.clone(),
                suggestion: finding.suggestion.clone(),
            },
        )?;
    Ok(())
}

pub(crate) fn render_consultation_details<W: Write>(
    consultation: &PendingConsultation,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<()> {
    let Some(finding) = consultation.hook_finding.as_ref() else {
        return Ok(());
    };
    let i18n = state.i18n();
    let created_at = consultation.created_at_ms.to_string();
    let user_interest_reason = user_interest_reason_for_consultation(consultation);
    let user_interest_reason_args = user_interest_reason.template_args(&i18n);
    let output_capture_status = if consultation.output_ref.is_some() {
        "captured"
    } else {
        "missing"
    };
    let mut body = vec![
        finding.description.clone(),
        i18n.format(
            cosh_shell::MessageId::HookDetailsConfidenceLine,
            &[
                ("confidence", consultation.confidence.as_str()),
                ("reason", consultation.display_reason.as_str()),
            ],
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsUserInterestLine,
            &user_interest_reason_args.as_template_args(),
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsTopicLine,
            &[
                ("topic", consultation.topic.as_str()),
                ("entity", consultation.entity_key.as_str()),
            ],
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsSuppressionKeyLine,
            &[("key", consultation.suppression_key.as_str())],
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsOutputRefLine,
            &[("ref", output_capture_status)],
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsCreatedAtLine,
            &[("created_at", created_at.as_str())],
        ),
        i18n.format(
            cosh_shell::MessageId::HookDetailsPromptHintLine,
            &[("hint", consultation.prompt_hint.as_str())],
        ),
    ];
    if let Some(skill) = consultation.recommended_skill.as_ref() {
        body.push(i18n.format(
            cosh_shell::MessageId::HookDetailsRecommendedSkillLine,
            &[("skill", skill.as_str())],
        ));
    }
    if let Some(cli_hint) = finding.cli_hint.as_ref() {
        body.push(i18n.format(
            cosh_shell::MessageId::HookDetailsReadOnlyCliHintLine,
            &[("hint", cli_hint.as_str())],
        ));
    }
    if state.debug {
        if let Some(output_ref) = consultation.output_ref.as_ref() {
            body.push(format!("debug_output_ref: {output_ref}"));
        }
    }
    RatatuiInlineRenderer::for_terminal().write_notice_panel(
        output,
        NoticePanelModel {
            title: i18n.t(cosh_shell::MessageId::HookFindingDetailsTitle),
            body,
            footer: Some(i18n.t(cosh_shell::MessageId::HookDetailsFooter)),
        },
    )
}

struct UserInterestReason {
    code: &'static str,
    description: cosh_shell::MessageId,
}

impl UserInterestReason {
    fn template_args<'a>(&'a self, i18n: &cosh_shell::I18n) -> UserInterestReasonTemplateArgs<'a> {
        UserInterestReasonTemplateArgs {
            code: self.code,
            description: i18n.t(self.description).to_string(),
        }
    }
}

struct UserInterestReasonTemplateArgs<'a> {
    code: &'a str,
    description: String,
}

impl<'a> UserInterestReasonTemplateArgs<'a> {
    fn as_template_args(&'a self) -> [(&'a str, &'a str); 2] {
        [
            ("code", self.code),
            ("description", self.description.as_str()),
        ]
    }
}

fn user_interest_reason_for_consultation(consultation: &PendingConsultation) -> UserInterestReason {
    let intent_reason = match classify_command_intent(&consultation.command) {
        CommandIntent::Lookup => Some(UserInterestReason {
            code: "lookup-intent",
            description: cosh_shell::MessageId::HookDetailsReasonLookupIntent,
        }),
        CommandIntent::Pipeline => Some(UserInterestReason {
            code: "pipeline-intent",
            description: cosh_shell::MessageId::HookDetailsReasonPipelineIntent,
        }),
        CommandIntent::Script => Some(UserInterestReason {
            code: "script-intent",
            description: cosh_shell::MessageId::HookDetailsReasonScriptIntent,
        }),
        CommandIntent::Wrapper => Some(UserInterestReason {
            code: "wrapper-low-confidence",
            description: cosh_shell::MessageId::HookDetailsReasonWrapperLowConfidence,
        }),
        CommandIntent::Interactive => Some(UserInterestReason {
            code: "interactive-intent",
            description: cosh_shell::MessageId::HookDetailsReasonInteractiveIntent,
        }),
        CommandIntent::Diagnostic | CommandIntent::Other => None,
    };
    if let Some(reason) = intent_reason {
        return reason;
    }

    match consultation.display_reason.as_str() {
        "active-agent-run-deferred" => UserInterestReason {
            code: "active-run-deferred",
            description: cosh_shell::MessageId::HookDetailsReasonActiveRunDeferred,
        },
        "user-continued-input" => UserInterestReason {
            code: "user-continued-input",
            description: cosh_shell::MessageId::HookDetailsReasonUserContinuedInput,
        },
        "non-diagnostic-success-command" => UserInterestReason {
            code: "non-diagnostic-success-command",
            description: cosh_shell::MessageId::HookDetailsReasonNonDiagnosticSuccessCommand,
        },
        "feedback-noisy" | "feedback-group-noisy" => UserInterestReason {
            code: "feedback-noisy",
            description: cosh_shell::MessageId::HookDetailsReasonFeedbackNoisy,
        },
        "ignored-same-finding" => UserInterestReason {
            code: "ignored-same-finding",
            description: cosh_shell::MessageId::HookDetailsReasonIgnoredSameFinding,
        },
        "same-card-already-rendered" => UserInterestReason {
            code: "same-card-already-rendered",
            description: cosh_shell::MessageId::HookDetailsReasonSameCardAlreadyRendered,
        },
        "interruption-budget" => UserInterestReason {
            code: "interruption-budget",
            description: cosh_shell::MessageId::HookDetailsReasonInterruptionBudget,
        },
        _ => {
            if consultation.display_reason == "low-confidence" {
                UserInterestReason {
                    code: "low-confidence",
                    description: cosh_shell::MessageId::HookDetailsReasonLowConfidence,
                }
            } else if matches!(
                classify_command_intent(&consultation.command),
                CommandIntent::Diagnostic
            ) {
                UserInterestReason {
                    code: "diagnostic-intent",
                    description: cosh_shell::MessageId::HookDetailsReasonDiagnosticIntent,
                }
            } else {
                UserInterestReason {
                    code: "other-intent",
                    description: cosh_shell::MessageId::HookDetailsReasonOtherIntent,
                }
            }
        }
    }
}
