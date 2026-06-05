use std::process::Command;

use crate::types::{AgentEvent, AgentRequest};

use super::{
    prompt_from_request, AdapterError, AgentAdapter, AgentBackendCapabilities, PreparedInvocation,
};

#[derive(Debug, Clone)]
pub struct QwenCliAdapter {
    pub program: String,
    pub allow_model_call: bool,
}

impl Default for QwenCliAdapter {
    fn default() -> Self {
        Self {
            program: "qwen".to_string(),
            allow_model_call: false,
        }
    }
}

impl QwenCliAdapter {
    pub fn prepare_invocation(&self, request: &AgentRequest) -> PreparedInvocation {
        PreparedInvocation {
            program: self.program.clone(),
            args: vec!["--approval-mode".to_string(), "plan".to_string()],
            prompt: prompt_from_request(request),
        }
    }
}

impl AgentAdapter for QwenCliAdapter {
    fn name(&self) -> &'static str {
        "qwen-cli"
    }

    fn capabilities(&self) -> AgentBackendCapabilities {
        AgentBackendCapabilities {
            text_stream: false,
            thinking_stream: false,
            session_resume: false,
            tool_intent: false,
            user_question: false,
            cancellable: false,
        }
    }

    fn run(&self, request: &AgentRequest) -> Result<Vec<AgentEvent>, AdapterError> {
        let prepared = self.prepare_invocation(request);
        if !self.allow_model_call {
            return Ok(qwen_dry_run_events(request, &prepared));
        }

        let output = Command::new(&prepared.program)
            .args(&prepared.args)
            .arg(&prepared.prompt)
            .output()
            .map_err(|err| AdapterError {
                message: format!("failed to run qwen cli: {err}"),
            })?;

        if !output.status.success() {
            return Ok(vec![AgentEvent::AgentFailed {
                run_id: request.id.clone(),
                error: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            }]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(vec![
            AgentEvent::TextDelta {
                run_id: request.id.clone(),
                text: stdout.trim().to_string(),
            },
            AgentEvent::AgentCompleted {
                run_id: request.id.clone(),
                summary: "qwen cli analysis completed".to_string(),
            },
        ])
    }
}

fn qwen_dry_run_events(request: &AgentRequest, prepared: &PreparedInvocation) -> Vec<AgentEvent> {
    let run_id = format!("qwen-dry-run-{}", request.command_block.id);
    vec![
        AgentEvent::StatusChanged {
            run_id: run_id.clone(),
            phase: "dry_run".to_string(),
            message: "prepared qwen cli invocation without model call".to_string(),
        },
        AgentEvent::TextDelta {
            run_id: run_id.clone(),
            text: format!(
                "Qwen CLI adapter prepared a safe recommend-only invocation; model execution is disabled by default.\n\nPrepared invocation:\n  {}",
                prepared.argv_preview().join(" ")
            ),
        },
        AgentEvent::AgentCompleted {
            run_id,
            summary: "qwen cli dry-run completed without model call".to_string(),
        },
    ]
}
