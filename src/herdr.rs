// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Deserialize;
use tokio::process::Command;

const HERDR_TIMEOUT: Duration = Duration::from_secs(3);

pub fn open() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build Herdr action runtime")?;
    runtime.block_on(open_async())
}

async fn open_async() -> anyhow::Result<()> {
    let herdr = required_absolute_path("HERDR_BIN_PATH")?;
    let pane = required_string("HERDR_PANE_ID")?;
    let socket = required_absolute_path("HERDR_SOCKET_PATH")?;
    let mut child = Command::new(herdr)
        .args([
            "plugin",
            "pane",
            "open",
            "--plugin",
            "local.agentsview",
            "--entrypoint",
            "activity",
            "--placement",
            "split",
            "--target-pane",
            &pane,
            "--direction",
            "right",
            "--focus",
        ])
        .env("HERDR_SOCKET_PATH", socket)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("start Herdr to open Activity plugin pane")?;
    let status = tokio::time::timeout(HERDR_TIMEOUT, child.wait())
        .await
        .context("open Activity plugin pane timed out after 3 seconds")?
        .context("wait for Herdr to open Activity plugin pane")?;
    if !status.success() {
        bail!("open Activity plugin pane failed with {status}");
    }
    Ok(())
}

pub fn resume_with(
    herdr: &Path,
    pane: &str,
    socket: &Path,
    command: &str,
    cwd: Option<&str>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build Herdr resume runtime")?;
    runtime.block_on(resume_async(herdr, pane, socket, command, cwd))
}

pub async fn resume(command: &str, cwd: Option<&str>) -> anyhow::Result<()> {
    resume_async(
        &required_absolute_path("HERDR_BIN_PATH")?,
        &required_string("HERDR_PANE_ID")?,
        &required_absolute_path("HERDR_SOCKET_PATH")?,
        command,
        cwd,
    )
    .await
}

async fn resume_async(
    herdr: &Path,
    pane: &str,
    socket: &Path,
    command: &str,
    cwd: Option<&str>,
) -> anyhow::Result<()> {
    if command.is_empty() {
        bail!("Herdr resume command is empty");
    }
    let mut split_args = vec!["pane", "split", "--pane", pane, "--direction", "right"];
    if let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) {
        split_args.extend(["--cwd", cwd]);
    }
    split_args.push("--focus");
    let split = run_herdr(herdr, socket, &split_args, true)
        .await
        .context("split a Herdr pane to resume the session")?;
    if !split.status.success() {
        bail!(
            "split a Herdr pane to resume the session failed with {}",
            split.status
        );
    }
    let pane_id = split_pane_id(&split.stdout)?;
    let run = run_herdr(herdr, socket, &["pane", "run", &pane_id, command], false)
        .await
        .context("run the resume command in a Herdr pane")?;
    if !run.status.success() {
        bail!(
            "run the resume command in a Herdr pane failed with {}",
            run.status
        );
    }
    Ok(())
}

async fn run_herdr(
    herdr: &Path,
    socket: &Path,
    args: &[&str],
    capture_stdout: bool,
) -> anyhow::Result<std::process::Output> {
    let child = Command::new(herdr)
        .args(args)
        .env("HERDR_SOCKET_PATH", socket)
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("start Herdr")?;
    tokio::time::timeout(HERDR_TIMEOUT, child.wait_with_output())
        .await
        .context("Herdr resume timed out after 3 seconds")?
        .context("wait for Herdr resume")
}

fn split_pane_id(stdout: &[u8]) -> anyhow::Result<String> {
    let response: PaneSplitResponse =
        serde_json::from_slice(stdout).context("parse Herdr pane split response")?;
    if response.result.pane.pane_id.is_empty() {
        bail!("Herdr pane split returned an empty pane id");
    }
    Ok(response.result.pane.pane_id)
}

#[derive(Deserialize)]
struct PaneSplitResponse {
    result: PaneSplitResult,
}

#[derive(Deserialize)]
struct PaneSplitResult {
    pane: PaneSplitPane,
}

#[derive(Deserialize)]
struct PaneSplitPane {
    pane_id: String,
}

fn required_string(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .with_context(|| format!("missing or invalid {name}"))
        .and_then(|value| {
            if value.is_empty() {
                bail!("{name} must not be empty");
            }
            Ok(value)
        })
}

fn required_absolute_path(name: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .with_context(|| format!("missing {name}"))?;
    if !path.is_absolute() {
        bail!("{name} must be an absolute path");
    }
    Ok(path)
}
