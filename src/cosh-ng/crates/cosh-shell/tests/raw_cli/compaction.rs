use super::*;

const SESSION_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Mock cosh-core covering all three invocation shapes the shell uses during
/// a compaction lifecycle: `--session-control` (resume validation), the
/// stream-json agent protocol, and the background `--compact` compactor.
struct CompactionFixture {
    home: std::path::PathBuf,
    workspace: std::path::PathBuf,
    core: std::path::PathBuf,
    compact_args: std::path::PathBuf,
    compact_pid: std::path::PathBuf,
}

impl CompactionFixture {
    /// `hang_compactor`: the `--compact` child traps SIGTERM and sleeps, so
    /// only the SIGKILL escalation can end it.
    fn new(label: &str, hang_compactor: bool) -> Self {
        let home = temp_shell_home(&format!("compaction-{label}"));
        let workspace = home.join("workspace");
        let bin = home.join("bin");
        fs::create_dir_all(&workspace).expect("create workspace");
        fs::create_dir_all(&bin).expect("create bin");
        let core = bin.join("cosh-core");
        let compact_args = home.join("compact-args.log");
        let compact_pid = home.join("compact.pid");
        let script = COMPACTION_CORE_SCRIPT
            .replace("__WORKSPACE__", &workspace.to_string_lossy())
            .replace("__COMPACT_ARGS__", &compact_args.to_string_lossy())
            .replace("__COMPACT_PID__", &compact_pid.to_string_lossy())
            .replace(
                "__COMPACT_MODE__",
                if hang_compactor { "hang" } else { "ok" },
            );
        write_executable(&core, &script);
        Self {
            home,
            workspace,
            core,
            compact_args,
            compact_pid,
        }
    }

    fn run(&self, chunks: Vec<(Vec<u8>, Duration)>) -> String {
        let home = self.home.to_string_lossy().into_owned();
        let core = self.core.to_string_lossy().into_owned();
        run_raw_cli_with_args_env_current_dir_and_delayed_input(
            "cosh-core",
            &[],
            &[
                ("HOME", &home),
                ("COSH_CORE_PATH", &core),
                // Deterministic English panel text regardless of host locale.
                ("LANG", "C.UTF-8"),
                ("LC_ALL", "C.UTF-8"),
            ],
            &self.workspace,
            chunks,
        )
    }
}

/// Real shell -> cosh-core manual compaction lifecycle: `/session compact`
/// returns the prompt immediately, ordinary shell commands keep working, the
/// Agent conversation is paused (queued), and once the background compactor
/// commits, the completion renders and the held Agent request resumes exactly
/// once.
#[test]
fn raw_cli_manual_compaction_keeps_shell_usable_and_resumes_agent() {
    let fixture = CompactionFixture::new("manual", false);
    let resume = format!("/session resume {SESSION_ID}\n");
    let output = fixture.run(vec![
        (resume.into_bytes(), Duration::ZERO),
        (b"/session compact\n".to_vec(), Duration::from_millis(700)),
        (
            b"echo during-compaction-works\n".to_vec(),
            Duration::from_millis(400),
        ),
        (
            b"?? follow-up while paused\n".to_vec(),
            Duration::from_millis(300),
        ),
        (b"echo tick-a\n".to_vec(), Duration::from_millis(400)),
        (b"echo tick-b\n".to_vec(), Duration::from_millis(2_200)),
        (b"exit\n".to_vec(), Duration::from_millis(1_200)),
    ]);

    // Compaction started in the background and the shell stayed usable.
    assert!(
        output.contains("Compaction is running in the background"),
        "{output}"
    );
    assert!(output.contains("during-compaction-works"), "{output}");
    // The Agent request submitted mid-compaction was paused, not started.
    assert!(
        output.contains("Agent paused during compaction") || output.contains("paused"),
        "{output}"
    );
    // The committed result rendered with the envelope's real numbers.
    assert!(output.contains("74210"), "{output}");
    assert!(output.contains("29800"), "{output}");
    // The compactor was spawned with the manual argument shape (no
    // auto-compact revision binding).
    let args = fs::read_to_string(&fixture.compact_args).expect("compact args recorded");
    assert!(args.contains("--compact"), "{args}");
    assert!(!args.contains("--auto-compact"), "{args}");
    assert!(args.contains(SESSION_ID), "{args}");
    // Shell command output precedes the completion notice: the compaction
    // never blocked the foreground shell.
    let shell_at = output
        .find("during-compaction-works")
        .expect("shell output");
    let done_at = output.find("74210").expect("completion");
    assert!(shell_at < done_at, "{output}");
    // The held Agent request resumed exactly once after completion.
    assert_eq!(
        output.matches("follow-up response done").count(),
        1,
        "{output}"
    );
    let _ = fs::remove_dir_all(&fixture.home);
}

/// Real automatic chain: the recommending agent process emits the terminal
/// Result JSON *before* the `compaction_recommended_v1` status (the adapter
/// must buffer the terminal event), never commits inline, and the shell
/// starts the background compactor at an idle boundary bound to the exact
/// generation/revision. The queued user request resumes exactly once after
/// the background commit.
#[test]
fn raw_cli_automatic_compaction_chain_binds_revision_and_resumes_once() {
    let fixture = CompactionFixture::new("auto", false);
    let resume = format!("/session resume {SESSION_ID}\n");
    let output = fixture.run(vec![
        (resume.into_bytes(), Duration::ZERO),
        (
            b"?? trigger-auto now\n".to_vec(),
            Duration::from_millis(700),
        ),
        (b"echo tick-1\n".to_vec(), Duration::from_millis(1_500)),
        (
            b"?? follow-up after compaction\n".to_vec(),
            Duration::from_millis(400),
        ),
        (b"echo tick-2\n".to_vec(), Duration::from_millis(500)),
        (b"echo tick-3\n".to_vec(), Duration::from_millis(2_200)),
        (b"exit\n".to_vec(), Duration::from_millis(1_200)),
    ]);

    // The recommendation (delivered after the buffered Result) started a
    // background compactor with the exact revision binding — the
    // recommending process itself committed nothing.
    let args = fs::read_to_string(&fixture.compact_args).expect("compact args recorded");
    assert!(args.contains("--auto-compact"), "{args}");
    assert!(args.contains("--expect-generation 1"), "{args}");
    assert!(args.contains("--expect-revision 0"), "{args}");
    assert!(args.contains(SESSION_ID), "{args}");
    // Auto-start notice frames the context pressure.
    assert!(
        output.contains("compaction is running in the background"),
        "{output}"
    );
    // Background commit rendered the envelope's numbers.
    assert!(output.contains("74210"), "{output}");
    assert!(output.contains("29800"), "{output}");
    // The user request queued behind the compaction resumed exactly once.
    assert_eq!(
        output.matches("follow-up response done").count(),
        1,
        "{output}"
    );
    let _ = fs::remove_dir_all(&fixture.home);
}

/// Cancellation against a compactor that ignores SIGTERM: the shell escalates
/// to SIGKILL after the grace period, reaps the child, renders the cancelled
/// completion, and the shell stays usable.
#[test]
fn raw_cli_cancel_escalates_to_sigkill_for_term_ignoring_compactor() {
    let fixture = CompactionFixture::new("cancel", true);
    let resume = format!("/session resume {SESSION_ID}\n");
    let mut chunks = vec![
        (resume.into_bytes(), Duration::ZERO),
        (b"/session compact\n".to_vec(), Duration::from_millis(700)),
        (
            b"/session compact cancel\n".to_vec(),
            Duration::from_millis(700),
        ),
    ];
    // Keep dispatch boundaries coming while the 5s SIGTERM->SIGKILL grace
    // period elapses; each tick drives one background poll.
    for index in 0..6 {
        chunks.push((
            format!("echo tick-{index}\n").into_bytes(),
            Duration::from_millis(1_200),
        ));
    }
    chunks.push((
        b"echo after-cancel-shell-ok\n".to_vec(),
        Duration::from_millis(600),
    ));
    chunks.push((b"exit\n".to_vec(), Duration::from_millis(400)));
    let output = fixture.run(chunks);

    assert!(output.contains("Cancellation requested"), "{output}");
    assert!(output.contains("cancelled"), "{output}");
    assert!(output.contains("after-cancel-shell-ok"), "{output}");
    // The TERM-ignoring compactor is gone: SIGKILL escalation reaped it.
    let pid: i32 = fs::read_to_string(&fixture.compact_pid)
        .expect("compactor pid recorded")
        .trim()
        .parse()
        .expect("parse compactor pid");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let alive = unsafe { nix::libc::kill(pid, 0) } == 0;
        if !alive {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "TERM-ignoring compactor {pid} survived cancellation"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_dir_all(&fixture.home);
}

const COMPACTION_CORE_SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = "--session-control" ]; then
  read -r request
  case "$request" in
    *'"action":"validate"'*)
      printf '%s\n' '{"ok":true,"data":{"action":"validate","session":{"session_id":"00000000-0000-4000-8000-000000000000","workspace_scope":"__WORKSPACE__","created_at_ms":1,"updated_at_ms":2,"model":"mock-history","message_count":6,"first_prompt":"first prompt","schema_version":1,"health":"ready"}}}'
      ;;
    *'"action":"list"'*)
      printf '%s\n' '{"ok":true,"data":{"action":"list","sessions":[{"session_id":"00000000-0000-4000-8000-000000000000","workspace_scope":"__WORKSPACE__","created_at_ms":1,"updated_at_ms":2,"model":"mock-history","message_count":6,"first_prompt":"first prompt","schema_version":1,"health":"ready"}],"next_cursor":null}}'
      ;;
    *'"action":"inspect"'*)
      printf '%s\n' '{"ok":true,"data":{"action":"inspect","session":{"session_id":"00000000-0000-4000-8000-000000000000","workspace_scope":"__WORKSPACE__","created_at_ms":1,"updated_at_ms":2,"model":"mock-history","message_count":6,"first_prompt":"first prompt","schema_version":1,"health":"ready"}}}'
      ;;
    *)
      printf '%s\n' '{"ok":false,"error":{"code":"corrupt","message":"unexpected request","recoverable":true,"hint":null}}'
      ;;
  esac
  exit 0
fi

case " $* " in
  *" --compact"*)
    printf '%s\n' "$*" >> "__COMPACT_ARGS__"
    printf '%s\n' "$$" > "__COMPACT_PID__"
    if [ "__COMPACT_MODE__" = "hang" ]; then
      trap '' TERM
      printf 'compactor fixture ignoring TERM\n' >&2
      i=0
      while [ "$i" -lt 300 ]; do
        sleep 0.1
        i=$((i + 1))
      done
      exit 1
    fi
    sleep 1
    printf '%s\n' '{"ok":true,"data":{"session_id":"00000000-0000-4000-8000-000000000000","revision":1,"compacted_through":4,"transcript_messages":6,"tokens_before":{"value":74210,"source":"provider_reported"},"tokens_after":{"value":29800,"source":"estimated"},"summary_bytes":128,"trigger":"manual"}}'
    exit 0
    ;;
esac

read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{"can_handle_can_use_tool":true,"can_handle_host_executed_shell_tool_result":true}}}}'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000000","model":"mock-history","tools":[]}'
read -r user_message
case "$user_message" in
  *trigger-auto*)
    printf '%s\n' '{"type":"assistant","session_id":"00000000-0000-4000-8000-000000000000","message":{"content":[{"type":"text","text":"auto trigger acknowledged"}]}}'
    # Terminal Result intentionally precedes the recommendation: the shell
    # adapter must buffer the terminal event so the recommendation is still
    # delivered to the runtime.
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"00000000-0000-4000-8000-000000000000","is_error":false,"duration_ms":1,"result":"done"}'
    printf '%s\n' '{"type":"system","subtype":"status","session_id":"00000000-0000-4000-8000-000000000000","status":"compaction_recommended_v1:00000000-0000-4000-8000-000000000000:1:0:200000:100000"}'
    ;;
  *follow-up*)
    printf '%s\n' '{"type":"assistant","session_id":"00000000-0000-4000-8000-000000000000","message":{"content":[{"type":"text","text":"follow-up response done"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"00000000-0000-4000-8000-000000000000","is_error":false,"duration_ms":1,"result":"done"}'
    ;;
  *)
    printf '%s\n' '{"type":"assistant","session_id":"00000000-0000-4000-8000-000000000000","message":{"content":[{"type":"text","text":"generic response"}]}}'
    printf '%s\n' '{"type":"result","subtype":"success","session_id":"00000000-0000-4000-8000-000000000000","is_error":false,"duration_ms":1,"result":"done"}'
    ;;
esac
"#;
