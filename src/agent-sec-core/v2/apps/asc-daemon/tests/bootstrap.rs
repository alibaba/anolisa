use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

mod support;

static DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct RunningBinary {
    child: Child,
    directory: PathBuf,
    socket_path: PathBuf,
}

impl Drop for RunningBinary {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if self.socket_path.exists() {
            let _ = std::fs::remove_file(&self.socket_path);
        }
        let _ = std::fs::remove_dir(&self.directory);
    }
}

fn unique_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "asc-daemon-bootstrap-{}-{}",
        std::process::id(),
        DIRECTORY_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

async fn wait_for_socket(path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("daemon bootstrap should bind its socket");
}

async fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("SIGTERM should stop the foreground daemon")
}

async fn request(path: &Path, payload: &[u8]) -> Value {
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream.write_all(payload).await.unwrap();
    let mut response = Vec::new();
    BufReader::new(stream)
        .read_until(b'\n', &mut response)
        .await
        .unwrap();
    assert_eq!(response.pop(), Some(b'\n'));
    serde_json::from_slice(&response).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dproc_002_003_and_partial_013_binary_registers_pap_and_cleans_socket() {
    let directory = unique_directory();
    std::fs::create_dir(&directory).unwrap();
    let socket_path = directory.join("daemon.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_asc-daemon"))
        .args(["serve", "--socket"])
        .arg(&socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut running = RunningBinary {
        child,
        directory,
        socket_path,
    };

    wait_for_socket(&running.socket_path).await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../crates/daemon/asc-daemon-protocol/tests/fixtures/pap-crud-e2e.json"
    ))
    .unwrap();
    if std::fs::metadata(&running.socket_path).unwrap().uid() == 0 {
        support::run_frozen_pap_crud_scenario(&running.socket_path, &fixture).await;
    } else {
        let first_request = fixture["steps"][0]["request"].clone();
        let mut payload = serde_json::to_vec(&first_request).unwrap();
        payload.push(b'\n');
        let response = request(&running.socket_path, &payload).await;
        assert_eq!(response["error"]["code"], "permission_denied");
    }

    let signal = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(running.child.id().to_string())
        .status()
        .unwrap();
    assert!(signal.success());
    let status = wait_for_exit(&mut running.child).await;

    assert!(status.success());
    assert!(!running.socket_path.exists());
}
