// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
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
        let executable = root.path().join("fake-herdr");
        let interpreter = option_env!("BASH_BIN_PATH").unwrap_or("/usr/bin/env bash");
        let sleep = option_env!("SLEEP_BIN_PATH").unwrap_or("sleep");
        let script =
            include_str!("bin/fake-herdr.sh").replacen("/usr/bin/env bash", interpreter, 1);
        let script = script.replace("@@SLEEP@@", sleep);
        fs::write(&executable, script).expect("write fake Herdr executable");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake Herdr metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake Herdr executable");
        Self { root, executable }
    }

    fn run(&self, mode: &str) -> Output {
        let mut command = Command::new(BINARY);
        command
            .arg("open")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("test PATH"))
            .env("HERDR_BIN_PATH", &self.executable)
            .env("HERDR_PANE_ID", "workspace-a:p3")
            .env("HERDR_SOCKET_PATH", "/tmp/fake-herdr.sock")
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

    fn child_pid(&self) -> u32 {
        fs::read_to_string(self.root.path().join("pid"))
            .expect("read fake Herdr pid")
            .trim()
            .parse()
            .expect("parse fake Herdr pid")
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
        .map(|entry| {
            entry
                .expect("read fake Herdr directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(created_names.len(), 2);
    assert!(created_names.iter().any(|name| name == "fake-herdr"));
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out after 3 seconds"));
    let pid = fake.child_pid();
    let deadline = Instant::now() + Duration::from_secs(1);
    while process_exists(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !process_exists(pid),
        "hung fake Herdr process {pid} survived"
    );
}

fn process_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
