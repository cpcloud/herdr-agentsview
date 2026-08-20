// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
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
        compile_fake("fake-herdr.rs", "fake_herdr", &executable);
        Self { root, executable }
    }

    fn run(&self, mode: &str) -> Output {
        self.command(mode)
            .output()
            .expect("run Activity open action")
    }

    fn command(&self, mode: &str) -> Command {
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
        command
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

struct FakeGit {
    _root: TempDir,
    path: std::ffi::OsString,
    cwd: PathBuf,
}

impl FakeGit {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fake Git directory");
        let executable = root
            .path()
            .join(format!("git{}", std::env::consts::EXE_SUFFIX));
        compile_fake("fake-git.rs", "fake_git", &executable);
        let cwd = root.path().join("worktree");
        fs::create_dir(&cwd).expect("create fake Git worktree");
        let mut paths = vec![root.path().to_owned()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").expect("test PATH"),
        ));
        let path = std::env::join_paths(paths).expect("build fake Git PATH");
        Self {
            _root: root,
            path,
            cwd,
        }
    }
}

fn compile_fake(source_name: &str, crate_name: &str, executable: &Path) {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/bin")
        .join(source_name);
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(compiler)
        .args([
            "--edition=2021",
            "--crate-name",
            crate_name,
            "-D",
            "warnings",
        ])
        .arg(source)
        .arg("-o")
        .arg(executable)
        .output()
        .expect("compile fake executable");
    assert!(
        output.status.success(),
        "compile {source_name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
        vec![
            vec![
                "pane".to_owned(),
                "get".to_owned(),
                "workspace-a:p3".to_owned(),
            ],
            vec![
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
            ],
        ]
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
fn open_passes_the_invoking_panes_repository_and_branch_to_the_plugin_pane() {
    // If source discovery or environment propagation disappears, Activity loses the exact
    // repository and branch that the invoking pane was working in.
    let fake = FakeHerdr::new();
    let git = FakeGit::new();
    let output = fake
        .command("success")
        .env("PATH", &git.path)
        .env("FAKE_GIT_CWD", &git.cwd)
        .env("FAKE_HERDR_FOREGROUND_CWD", &git.cwd)
        .output()
        .expect("run scoped Activity open action");

    assert!(
        output.status.success(),
        "open failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fake.calls().last().unwrap(),
        &vec![
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
            "--env".to_owned(),
            "HERDR_AGENTSVIEW_SOURCE_REMOTE=example.invalid/acme/project-alpha".to_owned(),
            "--env".to_owned(),
            "HERDR_AGENTSVIEW_SOURCE_BRANCH=feature/source-scope".to_owned(),
        ]
    );
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
