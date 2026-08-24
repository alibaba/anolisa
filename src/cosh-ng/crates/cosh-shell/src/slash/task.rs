//! Minimal persistent Task entrypoint backed by the local Gateway CLI.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use uuid::Uuid;

use crate::config::Language;
use crate::runtime::prelude::InlineState;
use crate::slash::panel::render_notice_panel;

const TASK_LIST_LIMIT: &str = "20";
const TASK_EVENT_LIMIT: &str = "64";
const MAX_TASK_EVENT_PAGES: usize = 256;
const MAX_RENDERED_RESULT_BYTES: usize = 64 * 1024;

pub(super) fn render_task_command<W: Write>(
    arguments: &str,
    state: &InlineState,
    output: &mut W,
) -> std::io::Result<bool> {
    let result = dispatch_task(arguments);
    let (title, body, footer) = match result {
        Ok(TaskDisplay::Submitted { task_id }) => (
            localized(state, "Persistent Task started", "持久 Task 已启动"),
            vec![
                format!("Task: {task_id}"),
                localized(
                    state,
                    "Codex continues under the local Gateway after this SSH session exits. Run /task later to check it.",
                    "退出当前 SSH 后，Codex 仍由本机 Gateway 托管。稍后重新进入 cosh，输入 /task 即可查看。",
                )
                .to_owned(),
            ],
            Some(localized(
                state,
                "Use /task show <task-id> for durable progress and results.",
                "使用 /task show <task-id> 查看持久进度与结果。",
            )),
        ),
        Ok(TaskDisplay::List(lines)) => (
            localized(state, "Persistent Tasks", "持久 Tasks"),
            lines,
            Some(localized(
                state,
                "Submit with /task <goal>; inspect with /task show [task-id].",
                "使用 /task <目标> 提交；使用 /task show [task-id] 查看。",
            )),
        ),
        Ok(TaskDisplay::Detail(lines)) => (
            localized(state, "Task result", "Task 结果"),
            lines,
            Some(localized(
                state,
                "This view is rebuilt from the Gateway's durable event cursor.",
                "此视图由 Gateway 的持久事件游标重建。",
            )),
        ),
        Err(error) => (
            localized(state, "Task unavailable", "Task 暂不可用"),
            vec![safe_text(&error)],
            Some(localized(
                state,
                "Make sure the delegated-acp-v1 Gateway service is running.",
                "请确认 delegated-acp-v1 Gateway 服务正在运行。",
            )),
        ),
    };
    render_notice_panel(output, title, body, footer)?;
    Ok(true)
}

enum TaskDisplay {
    Submitted { task_id: String },
    List(Vec<String>),
    Detail(Vec<String>),
}

fn dispatch_task(arguments: &str) -> Result<TaskDisplay, String> {
    let trimmed = arguments.trim();
    if trimmed.is_empty() || trimmed == "list" {
        return list_tasks().map(TaskDisplay::List);
    }
    if trimmed == "show" {
        let task_id = latest_task_id()?;
        return show_task(&task_id).map(TaskDisplay::Detail);
    }
    if let Some(task_id) = trimmed.strip_prefix("show ").map(str::trim) {
        if task_id.is_empty() || task_id.split_whitespace().count() != 1 {
            return Err("usage: /task show [task-id]".to_owned());
        }
        return show_task(task_id).map(TaskDisplay::Detail);
    }
    submit_task(trimmed).map(|task_id| TaskDisplay::Submitted { task_id })
}

fn submit_task(goal: &str) -> Result<String, String> {
    if goal.len() > 256 * 1024 {
        return Err("Task goal exceeds the 256 KiB limit".to_owned());
    }
    let key = format!("cosh-shell-task-{}", Uuid::new_v4());
    let output = run_gateway(
        &[
            "task",
            "--output",
            "jsonl",
            "submit",
            "--idempotency-key",
            &key,
            "--runtime",
            "acp",
            "--runtime-profile",
            "codex",
        ],
        Some(goal.as_bytes()),
    )?;
    let value = json_output(&output)?;
    value
        .get("task_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Gateway response did not contain a Task ID".to_owned())
}

fn list_tasks() -> Result<Vec<String>, String> {
    let output = run_gateway(
        &[
            "task",
            "--output",
            "jsonl",
            "list",
            "--limit",
            TASK_LIST_LIMIT,
        ],
        None,
    )?;
    let value = json_output(&output)?;
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| "Gateway response did not contain a Task list".to_owned())?;
    if tasks.is_empty() {
        return Ok(vec!["No persistent Tasks yet.".to_owned()]);
    }
    Ok(tasks
        .iter()
        .filter_map(|task| {
            Some(format!(
                "{}  {}  revision {}",
                task.get("task_id")?.as_str()?,
                task.get("state")?.as_str()?,
                task.get("revision")?.as_u64()?
            ))
        })
        .collect())
}

fn latest_task_id() -> Result<String, String> {
    let output = run_gateway(&["task", "--output", "jsonl", "list", "--limit", "1"], None)?;
    let value = json_output(&output)?;
    value
        .get("tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| tasks.first())
        .and_then(|task| task.get("task_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "No persistent Task is available".to_owned())
}

fn show_task(task_id: &str) -> Result<Vec<String>, String> {
    let projection = run_gateway(&["task", "--output", "jsonl", "get", task_id], None)?;
    let projection = json_output(&projection)?;
    let mut lines = vec![format!(
        "{}  {}  revision {}",
        projection
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or(task_id),
        projection
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        projection
            .get("revision")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )];
    let mut cursor = 0_u64;
    let mut transcript = TaskTranscript::default();
    let mut truncated = false;
    for page_index in 0..MAX_TASK_EVENT_PAGES {
        let cursor_text = cursor.to_string();
        let events = run_gateway(
            &[
                "task",
                "--output",
                "jsonl",
                "events",
                task_id,
                "--after",
                &cursor_text,
                "--limit",
                TASK_EVENT_LIMIT,
            ],
            None,
        )?;
        let events = json_output(&events)?;
        if let Some(items) = events.get("events").and_then(Value::as_array) {
            for item in items {
                let Some(event) = item.get("event") else {
                    continue;
                };
                transcript.record(event);
            }
        }
        let has_more = events.get("has_more").and_then(Value::as_bool) == Some(true);
        let next = events
            .get("next_revision")
            .and_then(Value::as_u64)
            .unwrap_or(cursor);
        if !has_more {
            break;
        }
        if next <= cursor {
            truncated = true;
            break;
        }
        if page_index + 1 == MAX_TASK_EVENT_PAGES {
            truncated = true;
            break;
        }
        cursor = next;
    }
    let (transcript_lines, transcript_truncated) = transcript.finish();
    lines.extend(transcript_lines);
    if truncated || transcript_truncated {
        lines.insert(
            1,
            "Some output was omitted; the durable event stream remains available.".to_owned(),
        );
    }
    Ok(lines)
}

#[derive(Default)]
struct TaskTranscript {
    lines: Vec<String>,
    pending: String,
    pending_run: Option<String>,
    rendered_bytes: usize,
    truncated: bool,
}

impl TaskTranscript {
    fn record(&mut self, event: &Value) {
        let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
        let update = event.get("update");
        let is_progress = event_name == "runtime_event_recorded"
            && update
                .and_then(|value| value.get("update"))
                .and_then(Value::as_str)
                == Some("progress");
        if is_progress {
            if let Some(summary) = update
                .and_then(|value| value.get("summary"))
                .and_then(Value::as_str)
            {
                let run_id = event.get("run_id").and_then(Value::as_str);
                if !self.pending.is_empty() && self.pending_run.as_deref() != run_id {
                    self.flush_progress();
                }
                self.pending_run = run_id.map(str::to_owned);
                self.append_progress(summary);
            }
            return;
        }

        self.flush_progress();
        if matches!(
            event_name,
            "task_succeeded" | "task_failed" | "task_cancelled" | "run_suspended"
        ) {
            self.lines.push(event_name.replace('_', " "));
        }
    }

    fn append_progress(&mut self, summary: &str) {
        let summary = safe_text(summary);
        let remaining = MAX_RENDERED_RESULT_BYTES.saturating_sub(self.rendered_bytes);
        if remaining == 0 {
            self.truncated |= !summary.is_empty();
            return;
        }
        let mut boundary = summary.len().min(remaining);
        while !summary.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        self.pending.push_str(&summary[..boundary]);
        self.rendered_bytes = self.rendered_bytes.saturating_add(boundary);
        self.truncated |= boundary < summary.len();
    }

    fn flush_progress(&mut self) {
        let paragraph = self
            .pending
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !paragraph.is_empty() {
            self.lines.push(paragraph);
        }
        self.pending.clear();
        self.pending_run = None;
    }

    fn finish(mut self) -> (Vec<String>, bool) {
        self.flush_progress();
        (self.lines, self.truncated)
    }
}

fn run_gateway(arguments: &[&str], stdin: Option<&[u8]>) -> Result<Output, String> {
    let program = gateway_executable()?;
    let mut command = Command::new(program);
    if arguments.first() == Some(&"task") {
        command.arg("task");
        if let Some(socket) = configured_gateway_socket()? {
            command.arg("--socket").arg(socket);
        }
        command.args(&arguments[1..]);
    } else {
        command.args(arguments);
    }
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to start cosh-gateway: {error}"))?;
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .ok_or_else(|| "Gateway stdin was unavailable".to_owned())?
            .write_all(input)
            .map_err(|error| format!("failed to submit Task goal: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to wait for cosh-gateway: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        let message = String::from_utf8_lossy(&output.stderr);
        Err(if message.trim().is_empty() {
            format!("cosh-gateway exited with {}", output.status)
        } else {
            message.trim().to_owned()
        })
    }
}

fn configured_gateway_socket() -> Result<Option<PathBuf>, String> {
    if let Some(configured) = std::env::var_os("COSH_GATEWAY_SOCKET") {
        let configured = PathBuf::from(configured);
        if !configured.is_absolute() {
            return Err("COSH_GATEWAY_SOCKET must be absolute".to_owned());
        }
        return Ok(Some(configured));
    }
    let Some(user) = std::env::var_os("USER") else {
        return Ok(None);
    };
    let user = user
        .to_str()
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
        .ok_or_else(|| "USER cannot name a Gateway runtime directory".to_owned())?;
    let packaged = PathBuf::from(format!("/run/cosh-gateway-{user}/gateway.sock"));
    Ok(packaged.exists().then_some(packaged))
}

fn gateway_executable() -> Result<PathBuf, String> {
    if let Some(configured) = std::env::var_os("COSH_GATEWAY_EXECUTABLE") {
        let configured = PathBuf::from(configured);
        if !configured.is_absolute() {
            return Err("COSH_GATEWAY_EXECUTABLE must be absolute".to_owned());
        }
        return Ok(configured);
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            let sibling = parent.join("cosh-gateway");
            if is_executable_file(&sibling) {
                return Ok(sibling);
            }
        }
    }
    Ok(PathBuf::from("cosh-gateway"))
}

fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn json_output(output: &Output) -> Result<Value, String> {
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| "Gateway returned non-UTF-8 output".to_owned())?;
    text.lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| "Gateway returned no JSON result".to_owned())
}

fn safe_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>()
        .replace(['\n', '\t'], " ")
}

fn localized<'a>(state: &InlineState, english: &'a str, chinese: &'a str) -> &'a str {
    match state.language {
        Language::EnUs => english,
        Language::ZhCn => chinese,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    use super::{list_tasks, safe_text, show_task, submit_task};

    struct GatewayEnvironment;

    impl GatewayEnvironment {
        fn set(executable: &std::path::Path) -> Self {
            std::env::set_var("COSH_GATEWAY_EXECUTABLE", executable);
            std::env::set_var("COSH_GATEWAY_SOCKET", "/tmp/cosh-test-gateway.sock");
            Self
        }
    }

    impl Drop for GatewayEnvironment {
        fn drop(&mut self) {
            std::env::remove_var("COSH_GATEWAY_EXECUTABLE");
            std::env::remove_var("COSH_GATEWAY_SOCKET");
        }
    }

    fn environment_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn task_submit_and_list_use_the_delegated_codex_defaults() {
        let _lock = environment_lock();
        let root = tempfile::tempdir().unwrap();
        let gateway = root.path().join("cosh-gateway");
        let argv = root.path().join("argv");
        let goal = root.path().join("goal");
        fs::write(
            &gateway,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\ncase \" $* \" in *' submit '*) cat > '{}'; printf '%s\\n' '{{\"event\":\"task\",\"task_id\":\"tsk_00000000-0000-0000-0000-000000000001\"}}' ;; *) printf '%s\\n' '{{\"event\":\"tasks\",\"tasks\":[{{\"task_id\":\"tsk_00000000-0000-0000-0000-000000000001\",\"state\":\"succeeded\",\"revision\":7}}]}}' ;; esac\n",
                argv.display(),
                goal.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&gateway, fs::Permissions::from_mode(0o700)).unwrap();
        let _environment = GatewayEnvironment::set(&gateway);

        let task_id = submit_task("update dependencies").unwrap();
        assert_eq!(task_id, "tsk_00000000-0000-0000-0000-000000000001");
        assert_eq!(fs::read_to_string(&goal).unwrap(), "update dependencies");
        let submitted_args = fs::read_to_string(&argv).unwrap();
        assert!(submitted_args.contains("--runtime acp"), "{submitted_args}");
        assert!(
            submitted_args.contains("--runtime-profile codex"),
            "{submitted_args}"
        );
        assert!(
            submitted_args.contains("--socket /tmp/cosh-test-gateway.sock"),
            "{submitted_args}"
        );

        assert_eq!(
            list_tasks().unwrap(),
            ["tsk_00000000-0000-0000-0000-000000000001  succeeded  revision 7"]
        );
    }

    #[test]
    fn task_show_replays_every_durable_event_page() {
        let _lock = environment_lock();
        let root = tempfile::tempdir().unwrap();
        let gateway = root.path().join("cosh-gateway");
        fs::write(
            &gateway,
            r#"#!/bin/sh
case " $* " in
  *' get '*)
    printf '%s\n' '{"event":"task","task_id":"tsk_00000000-0000-0000-0000-000000000001","state":"succeeded","revision":5}'
    ;;
  *' events '*' --after 0 '*)
    printf '%s\n' '{"event":"task_events","events":[{"revision":1,"event":{"event":"runtime_event_recorded","run_id":"run-1","update":{"update":"progress","summary":"我"}}},{"revision":2,"event":{"event":"runtime_event_recorded","run_id":"run-1","update":{"update":"progress","summary":"会"}}}],"has_more":true,"next_revision":2}'
    ;;
  *' events '*' --after 2 '*)
    printf '%s\n' '{"event":"task_events","events":[{"revision":3,"event":{"event":"runtime_event_recorded","run_id":"run-1","update":{"update":"progress","summary":"读取"}}},{"revision":4,"event":{"event":"runtime_event_recorded","run_id":"run-1","update":{"update":"progress","summary":"文件"}}},{"revision":5,"event":{"event":"task_succeeded"}}],"has_more":false,"next_revision":5}'
    ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&gateway, fs::Permissions::from_mode(0o700)).unwrap();
        let _environment = GatewayEnvironment::set(&gateway);

        assert_eq!(
            show_task("tsk_00000000-0000-0000-0000-000000000001").unwrap(),
            [
                "tsk_00000000-0000-0000-0000-000000000001  succeeded  revision 5",
                "我会读取文件",
                "task succeeded",
            ]
        );
    }

    #[test]
    fn task_transcript_breaks_paragraphs_at_non_progress_events() {
        let mut transcript = super::TaskTranscript::default();
        for event in [
            serde_json::json!({
                "event": "runtime_event_recorded",
                "run_id": "run-1",
                "update": {"update": "progress", "summary": "Run"}
            }),
            serde_json::json!({
                "event": "runtime_event_recorded",
                "run_id": "run-1",
                "update": {"update": "progress", "summary": " tests"}
            }),
            serde_json::json!({"event": "approval_requested"}),
            serde_json::json!({
                "event": "runtime_event_recorded",
                "run_id": "run-1",
                "update": {"update": "progress", "summary": "Done\ncleanly"}
            }),
            serde_json::json!({"event": "task_succeeded"}),
        ] {
            transcript.record(&event);
        }
        assert_eq!(
            transcript.finish().0,
            ["Run tests", "Done cleanly", "task succeeded"]
        );
    }

    #[test]
    fn task_result_text_drops_terminal_controls() {
        assert_eq!(safe_text("ok\u{1b}[31m\nnext"), "ok[31m next");
    }
}
