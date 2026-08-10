use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context};
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
