use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;

use super::CoshCoreAdapter;

/// Default timeout for registry queries (5 seconds).
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(5);

impl CoshCoreAdapter {
    /// Synchronous registry query: spawns a short-lived `cosh-core --registry` process,
    /// sends one registry_request via stdin, reads one registry_response from stdout.
    pub fn registry_query(
        &self,
        domain: &str,
        action: &str,
        params: Value,
    ) -> Result<Value, String> {
        let request_id = format!("reg-{}", std::process::id());
        let request = serde_json::json!({
            "type": "registry_request",
            "request_id": request_id,
            "domain": domain,
            "action": action,
            "params": params,
        });

        let request_json =
            serde_json::to_string(&request).map_err(|e| format!("serialize error: {e}"))?;

        let mut child = Command::new(&self.program)
            .arg("--registry")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to spawn cosh-core --registry: {e}"))?;

        // Write request to stdin
        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| "failed to open stdin".to_string())?;
            writeln!(stdin, "{request_json}").map_err(|e| format!("write error: {e}"))?;
            // Drop stdin to signal EOF
        }
        drop(child.stdin.take());

        // Read response from stdout with timeout
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "failed to open stdout".to_string())?;

        let (tx, rx) = std::sync::mpsc::channel();
        let reader_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if !l.trim().is_empty() => {
                        let _ = tx.send(Ok(l));
                        return;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        let _ = tx.send(Err(format!("read error: {e}")));
                        return;
                    }
                }
            }
            let _ = tx.send(Err("no response received (EOF)".to_string()));
        });

        let response_line = match rx.recv_timeout(REGISTRY_TIMEOUT) {
            Ok(Ok(line)) => line,
            Ok(Err(e)) => {
                let _ = child.kill();
                return Err(e);
            }
            Err(_) => {
                let _ = child.kill();
                return Err("registry query timed out".to_string());
            }
        };

        let _ = reader_handle.join();
        let _ = child.wait();

        // Parse the response
        let resp: Value =
            serde_json::from_str(&response_line).map_err(|e| format!("parse error: {e}"))?;

        let success = resp
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if success {
            Ok(resp.get("data").cloned().unwrap_or(Value::Null))
        } else {
            let error = resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error")
                .to_string();
            Err(error)
        }
    }
}
