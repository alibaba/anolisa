use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::types::ShellEvent;

pub(crate) mod audit;

pub fn write_shell_events(path: impl AsRef<Path>, events: &[ShellEvent]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    let mut writer = BufWriter::new(file);
    for event in events {
        let event = redacted_event(event);
        serde_json::to_writer(&mut writer, &event).map_err(json_to_io)?;
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

pub(crate) fn redacted_shell_events(events: &[ShellEvent]) -> Vec<ShellEvent> {
    events.iter().map(redacted_event).collect()
}

fn redacted_event(event: &ShellEvent) -> ShellEvent {
    let mut event = event.clone();
    event.session_id = redact(&event.session_id);
    event.command_id = event.command_id.as_deref().map(redact);
    event.cwd = event.cwd.as_deref().map(redact);
    event.end_cwd = event.end_cwd.as_deref().map(redact);
    event.terminal_output_ref = event.terminal_output_ref.as_deref().map(redact);
    if event.component.as_deref() == Some("card_secret") {
        event.input = event.input.as_ref().map(|_| "<redacted>".to_string());
    } else {
        event.input = event.input.as_deref().map(redact);
    }
    event.command = event.command.as_deref().map(redact);
    event.component = event.component.as_deref().map(redact);
    event.message = event.message.as_deref().map(redact);
    event
}

fn redact(value: &str) -> String {
    crate::evidence::redact_sensitive_text(value).0
}

pub fn read_shell_events(path: impl AsRef<Path>) -> io::Result<Vec<ShellEvent>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        events.push(serde_json::from_str(&line).map_err(json_to_io)?);
    }

    Ok(events)
}

fn json_to_io(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use super::{read_shell_events, write_shell_events};
    use crate::types::ShellEvent;

    #[test]
    fn journal_redacts_commands_prompts_and_secret_card_input() {
        let path = std::env::temp_dir().join(format!(
            "cosh-shell-secret-journal-{}-{}.jsonl",
            std::process::id(),
            now_nanos()
        ));
        let command_secret = "cli-secret-value";
        let prompt_secret = "ghp_abcdefghijklmnopqrstuvwxyz123456";
        let auth_secret = "short-auth-value";
        let mut prompt = ShellEvent::user_input_intercepted(
            "session-1",
            format!("?? inspect token={prompt_secret}"),
        );
        prompt.component = Some("agent_marker".to_string());
        let mut auth =
            ShellEvent::user_input_intercepted("session-1", format!("auth-1:{auth_secret}"));
        auth.component = Some("card_secret".to_string());
        auth.message = Some("input".to_string());
        let mut path_event = ShellEvent::command_started(
            "session-token=session-secret",
            "command-token=command-id-secret",
            "safe",
            "/tmp/token=cwd-secret",
            2,
        );
        path_event.end_cwd = Some("/tmp/token=end-cwd-secret".to_string());
        path_event.terminal_output_ref = Some("/tmp/token=output-ref-secret".to_string());

        write_shell_events(
            &path,
            &[
                ShellEvent::command_started(
                    "session-1",
                    "command-1",
                    format!("curl --token {command_secret}"),
                    "/tmp",
                    1,
                ),
                prompt,
                auth,
                path_event,
            ],
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        for secret in [
            command_secret,
            prompt_secret,
            auth_secret,
            "session-secret",
            "command-id-secret",
            "cwd-secret",
            "end-cwd-secret",
            "output-ref-secret",
        ] {
            assert!(!content.contains(secret), "{content}");
        }

        let events = read_shell_events(&path).unwrap();
        assert_eq!(
            events[0].command.as_deref(),
            Some("curl --token <redacted>")
        );
        assert!(events[1]
            .input
            .as_deref()
            .is_some_and(|input| input.contains("token=<redacted>")));
        assert_eq!(events[2].input.as_deref(), Some("<redacted>"));
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn journal_uses_private_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let path = std::env::temp_dir().join(format!(
            "cosh-shell-private-journal-{}-{}.jsonl",
            std::process::id(),
            now_nanos()
        ));

        write_shell_events(&path, &[]).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(path);
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }
}
