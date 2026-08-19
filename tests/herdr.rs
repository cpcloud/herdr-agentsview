// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_herdr-agentsview");

struct FakeHerdr {
    root: TempDir,
    executable: PathBuf,
}

impl FakeHerdr {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fake Herdr directory");
        let binary_dir = root.path().join("bin");
        fs::create_dir(&binary_dir).expect("create fake Herdr binary directory");
        let executable = binary_dir.join(format!("fake-herdr{}", std::env::consts::EXE_SUFFIX));
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bin/fake-herdr.rs");
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(compiler)
            .args([
                "--edition=2021",
                "--crate-name",
                "fake_herdr",
                "-D",
                "warnings",
            ])
            .arg(source)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("compile fake Herdr executable");
        assert!(
            output.status.success(),
            "compile fake Herdr executable: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self { root, executable }
    }

    fn run(&self, mode: &str) -> Output {
        let mut command = Command::new(BINARY);
        command
            .arg("open")
            .env("PATH", std::env::var_os("PATH").expect("test PATH"))
            .env("HERDR_BIN_PATH", &self.executable)
            .env("HERDR_PANE_ID", "workspace-a:p3")
            .env(
                "HERDR_SOCKET_PATH",
                self.root.path().join("fake-herdr.sock"),
            )
            .env("FAKE_HERDR_DIR", self.root.path())
            .env("FAKE_HERDR_MODE", mode);
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().expect("run Activity open action")
    }

    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.path().join("calls"))
            .expect("read fake Herdr calls")
            .lines()
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect()
    }

    fn hang_endpoint(&self) -> SocketAddr {
        fs::read_to_string(self.root.path().join("endpoint"))
            .expect("read fake Herdr endpoint")
            .trim()
            .parse()
            .expect("parse fake Herdr endpoint")
    }
}

#[test]
fn open_targets_the_invoking_pane_without_writing_plugin_state() {
    // If open regresses to durable pane bookkeeping or loses the invoking pane, the Activity
    // action can leak state or place the dashboard in the wrong workspace.
    let fake = FakeHerdr::new();

    let output = fake.run("success");

    assert!(
        output.status.success(),
        "open failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fake.calls(),
        vec![vec![
            "plugin".to_owned(),
            "pane".to_owned(),
            "open".to_owned(),
            "--plugin".to_owned(),
            "local.agentsview".to_owned(),
            "--entrypoint".to_owned(),
            "activity".to_owned(),
            "--placement".to_owned(),
            "split".to_owned(),
            "--target-pane".to_owned(),
            "workspace-a:p3".to_owned(),
            "--direction".to_owned(),
            "right".to_owned(),
            "--focus".to_owned(),
        ]]
    );
    let created_names = fs::read_dir(fake.root.path())
        .expect("list fake Herdr directory")
        .map(|entry| entry.expect("read fake Herdr directory entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(created_names.len(), 2);
    assert!(created_names.iter().any(|name| name == "bin"));
    assert!(created_names.iter().any(|name| name == "calls"));
}

#[test]
fn open_propagates_a_failed_herdr_invocation() {
    // If Herdr rejects the pane request, a success exit would hide a broken plugin action.
    let fake = FakeHerdr::new();

    let output = fake.run("failure");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("open Activity plugin pane"));
}

#[test]
fn open_times_out_and_terminates_a_hung_herdr_process() {
    // If timeout supervision regresses, invoking the Activity action can hang indefinitely or
    // leave a detached Herdr process after the caller returns.
    let fake = FakeHerdr::new();
    let started_at = Instant::now();

    let output = fake.run("hang");

    assert!(!output.status.success());
    assert!(started_at.elapsed() < Duration::from_secs(5));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out after 3 seconds"),
        "unexpected Activity open error: {stderr}"
    );
    let endpoint = fake.hang_endpoint();
    let deadline = Instant::now() + Duration::from_secs(1);
    while endpoint_accepts_connections(endpoint) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !endpoint_accepts_connections(endpoint),
        "hung fake Herdr process still listens on {endpoint}"
    );
}

fn endpoint_accepts_connections(endpoint: SocketAddr) -> bool {
    TcpStream::connect_timeout(&endpoint, Duration::from_millis(50)).is_ok()
}
