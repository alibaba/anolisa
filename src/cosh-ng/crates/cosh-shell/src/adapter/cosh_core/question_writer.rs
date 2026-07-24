use std::io::Write;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::super::{
    control_protocol, AdapterError, ApprovalDecision, ApprovalResponse, AuthResponse,
};
use super::question_ingress::{protocol_error, CoreQuestionProtocolReason, CoshCoreQuestionGate};

pub(crate) struct QuestionWriter {
    pub(crate) stdin: ChildStdin,
    pub(crate) prompt: String,
    pub(crate) approval_rx: mpsc::Receiver<ApprovalResponse>,
    pub(crate) auth_rx: mpsc::Receiver<AuthResponse>,
    pub(crate) done: Arc<AtomicBool>,
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) gate: Arc<Mutex<CoshCoreQuestionGate>>,
    pub(crate) failure_tx: mpsc::Sender<AdapterError>,
    pub(crate) answer_confirmation_tx: mpsc::Sender<Result<String, AdapterError>>,
}

impl QuestionWriter {
    pub(crate) fn spawn(self) -> thread::JoinHandle<()> {
        thread::spawn(move || self.run())
    }

    fn run(self) {
        let mut writer = std::io::BufWriter::new(self.stdin);
        let init_msg = control_protocol::serialize_initialize("init-1");
        let _ = writeln!(writer, "{init_msg}");
        let _ = writer.flush();

        if !self.prompt.is_empty() {
            let user_msg = control_protocol::serialize_user_message(&self.prompt, None);
            let _ = writeln!(writer, "{user_msg}");
            let _ = writer.flush();
        }

        while !self.done.load(Ordering::SeqCst) && !self.cancelled.load(Ordering::SeqCst) {
            let (message, answered_request_id) =
                match Self::next_message(&self.approval_rx, &self.auth_rx) {
                    Ok(Some(message)) => message,
                    Ok(None) => continue,
                    Err(()) => break,
                };
            if let Some(request_id) = answered_request_id {
                if write_answer(&mut writer, &message, &request_id, &self.gate).is_err() {
                    let error = protocol_error(CoreQuestionProtocolReason::AnswerWriteFailed);
                    let _ = self.answer_confirmation_tx.send(Err(error.clone()));
                    let _ = self.failure_tx.send(error);
                    break;
                }
                let _ = self.answer_confirmation_tx.send(Ok(request_id));
            } else if writeln!(writer, "{message}").is_err() || writer.flush().is_err() {
                break;
            }
        }
    }

    fn next_message(
        approval_rx: &mpsc::Receiver<ApprovalResponse>,
        auth_rx: &mpsc::Receiver<AuthResponse>,
    ) -> Result<Option<(String, Option<String>)>, ()> {
        match approval_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(response) => {
                let answered_request_id =
                    matches!(response.decision, ApprovalDecision::Answer { .. })
                        .then(|| response.request_id.clone());
                let message = match &response.decision {
                    ApprovalDecision::Allow => {
                        control_protocol::serialize_co_allow(&response.request_id)
                    }
                    ApprovalDecision::Deny { message } => {
                        control_protocol::serialize_deny(&response.request_id, message)
                    }
                    ApprovalDecision::HostExecutedShell { result } => {
                        control_protocol::serialize_host_executed_shell_result(
                            &response.request_id,
                            result,
                        )
                    }
                    ApprovalDecision::Answer { answer } => {
                        control_protocol::serialize_answer(&response.request_id, answer)
                    }
                    ApprovalDecision::ShellEvidence { result } => {
                        control_protocol::serialize_shell_evidence_result(
                            &response.request_id,
                            result,
                        )
                    }
                };
                Ok(Some((message, answered_request_id)))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => match auth_rx.try_recv() {
                Ok(response) => Ok(Some((
                    control_protocol::serialize_auth_response(
                        &response.request_id,
                        &response.provider_id,
                        response.provider_type.as_deref(),
                        &response.values,
                        response.persist,
                        response.reset_unavailable_credentials,
                    ),
                    None,
                ))),
                Err(mpsc::TryRecvError::Empty) => Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => Err(()),
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(()),
        }
    }
}

fn write_answer<W: Write>(
    writer: &mut W,
    message: &str,
    request_id: &str,
    gate: &Arc<Mutex<CoshCoreQuestionGate>>,
) -> Result<(), ()> {
    let mut gate = gate.lock().map_err(|_| ())?;
    writeln!(writer, "{message}").map_err(|_| ())?;
    writer.flush().map_err(|_| ())?;
    gate.answer_written(request_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::question_ingress::{
        classify_output_line, CoshCoreOutputClass, QuestionGateDecision,
    };
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct GateProbeWriter {
        gate: Arc<Mutex<CoshCoreQuestionGate>>,
        saw_gate_locked: Arc<AtomicBool>,
    }

    impl Write for GateProbeWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.saw_gate_locked
                .store(self.gate.try_lock().is_err(), Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn answer_write_holds_question_gate_until_flush_and_clear_are_atomic() {
        let gate = Arc::new(Mutex::new(CoshCoreQuestionGate::default()));
        let question = match classify_output_line(
            r#"{"type":"control_request","request_id":"q1","request":{"subtype":"ask_user","question":"First?"}}"#,
        )
        .unwrap()
        {
            CoshCoreOutputClass::ValidAskUser(question) => question,
            CoshCoreOutputClass::PassThrough => panic!("expected question"),
        };
        assert_eq!(
            gate.lock().unwrap().accept(&question).unwrap(),
            QuestionGateDecision::Accept
        );
        let saw_gate_locked = Arc::new(AtomicBool::new(false));
        let mut writer = GateProbeWriter {
            gate: Arc::clone(&gate),
            saw_gate_locked: Arc::clone(&saw_gate_locked),
        };

        write_answer(&mut writer, "answer", "q1", &gate).unwrap();

        assert!(saw_gate_locked.load(Ordering::SeqCst));
    }
}
