// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context};
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use tokio::process::Command;
use url::Url;

const HERDR_TIMEOUT: Duration = Duration::from_secs(3);
pub const SOURCE_REMOTE_ENV: &str = "HERDR_AGENTSVIEW_SOURCE_REMOTE";
pub const SOURCE_BRANCH_ENV: &str = "HERDR_AGENTSVIEW_SOURCE_BRANCH";

#[derive(Debug)]
struct LaunchContext {
    normalized_remote: String,
    branch: String,
}

#[derive(Deserialize)]
struct PaneResponse {
    result: PaneResult,
}

#[derive(Deserialize)]
struct PaneResult {
    pane: PaneInfo,
}

#[derive(Deserialize)]
struct PaneInfo {
    cwd: String,
    foreground_cwd: Option<String>,
}

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
    let launch_context = discover_launch_context(&herdr, &pane, &socket).await;
    let mut command = Command::new(herdr);
    command
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
        .kill_on_drop(true);
    if let Some(context) = launch_context {
        command
            .arg("--env")
            .arg(format!("{SOURCE_REMOTE_ENV}={}", context.normalized_remote))
            .arg("--env")
            .arg(format!("{SOURCE_BRANCH_ENV}={}", context.branch));
    }
    let mut child = command
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

async fn discover_launch_context(herdr: &Path, pane: &str, socket: &Path) -> Option<LaunchContext> {
    let output = tokio::time::timeout(
        HERDR_TIMEOUT,
        Command::new(herdr)
            .args(["pane", "get", pane])
            .env("HERDR_SOCKET_PATH", socket)
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let pane = serde_json::from_slice::<PaneResponse>(&output.stdout)
        .ok()?
        .result
        .pane;
    let cwd = pane
        .foreground_cwd
        .filter(|cwd| !cwd.is_empty())
        .unwrap_or(pane.cwd);
    let cwd = PathBuf::from(cwd);
    if !cwd.is_absolute() {
        return None;
    }

    let branch = git_output(&cwd, ["symbolic-ref", "--quiet", "--short", "HEAD"]).await?;
    let remote_names = git_output(&cwd, ["remote"]).await?;
    let mut normalized = Vec::new();
    for name in remote_names.lines().filter(|name| !name.is_empty()) {
        let raw = git_output(&cwd, ["remote", "get-url", name]).await?;
        if let Some(remote) = normalize_git_remote(&raw) {
            if name == "origin" {
                return Some(LaunchContext {
                    normalized_remote: remote,
                    branch,
                });
            }
            normalized.push(remote);
        }
    }
    let normalized = normalized.into_iter().collect::<BTreeSet<_>>();
    if normalized.len() != 1 {
        return None;
    }
    Some(LaunchContext {
        normalized_remote: normalized.into_iter().next()?,
        branch,
    })
}

async fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    let output = tokio::time::timeout(
        HERDR_TIMEOUT,
        Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn normalize_git_remote(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.starts_with('/')
        || raw
            .get(1..3)
            .is_some_and(|value| value == ":/" || value == ":\\")
        || raw.to_ascii_lowercase().starts_with("file://")
    {
        return None;
    }
    let (host, port, repo_path) = if raw.contains("://") {
        let url = Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "ssh" | "git" | "https" | "http") {
            return None;
        }
        let port = url.port().filter(|port| {
            !matches!(
                (url.scheme(), *port),
                ("ssh", 22) | ("git", 9418) | ("https", 443) | ("http", 80)
            )
        });
        let repo_path = percent_decode_str(url.path())
            .decode_utf8()
            .ok()?
            .into_owned();
        (url.host_str()?.to_owned(), port, repo_path)
    } else {
        let without_user = raw.rsplit_once('@').map_or(raw, |(_, value)| value);
        let (host, repo_path) = without_user.split_once(':')?;
        let repo_path = repo_path
            .split_once(['?', '#'])
            .map_or(repo_path, |(path, _)| path);
        (host.to_owned(), None, repo_path.to_owned())
    };
    let mut host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        host = match address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
    }
    if let Some(port) = port {
        host = format!("{host}:{port}");
    }
    let repo_path = normalize_repo_path(&repo_path)?;
    Some(format!("{host}/{repo_path}"))
}

fn normalize_repo_path(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let mut components = Vec::new();
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            value => components.push(value),
        }
    }
    let last = components.last_mut()?;
    *last = last.strip_suffix(".git").unwrap_or(last);
    if last.is_empty() {
        return None;
    }
    Some(components.join("/"))
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
