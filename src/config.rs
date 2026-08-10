use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context};
use chrono_tz::Tz;
use secrecy::SecretString;
use serde::Deserialize;
use url::{Host, Url};

pub struct PluginConfig {
    pub api_base_url: Url,
    pub request_timeout: Option<Duration>,
    pub refresh_interval: Duration,
    pub timezone: Tz,
    pub auth: Option<SecretString>,
}

impl fmt::Debug for PluginConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginConfig")
            .field("api_base_url", &self.api_base_url)
            .field("request_timeout", &self.request_timeout)
            .field("refresh_interval", &self.refresh_interval)
            .field("timezone", &self.timezone)
            .field("has_auth", &self.auth.is_some())
            .finish()
    }
}

pub(crate) struct ConfigEnvironment {
    pub(crate) plugin_config_dir: Option<PathBuf>,
    pub(crate) tz: Option<OsString>,
    pub(crate) token: Option<OsString>,
    pub(crate) token_file: Option<PathBuf>,
    pub(crate) os_timezone: Result<String, String>,
}

impl ConfigEnvironment {
    fn from_process() -> Self {
        Self {
            plugin_config_dir: std::env::var_os("HERDR_PLUGIN_CONFIG_DIR").map(PathBuf::from),
            tz: std::env::var_os("TZ"),
            token: std::env::var_os("AGENTSVIEW_TOKEN"),
            token_file: std::env::var_os("AGENTSVIEW_TOKEN_FILE").map(PathBuf::from),
            os_timezone: iana_time_zone::get_timezone().map_err(|error| error.to_string()),
        }
    }

    fn config_path(&self) -> anyhow::Result<PathBuf> {
        let directory = self.plugin_config_dir.as_ref().context(
            "HERDR_PLUGIN_CONFIG_DIR is unset; point it at an Activity config directory",
        )?;
        if !directory.is_absolute() {
            bail!("HERDR_PLUGIN_CONFIG_DIR must be an absolute path");
        }
        let metadata = fs::metadata(directory)
            .with_context(|| format!("inspect HERDR_PLUGIN_CONFIG_DIR {}", directory.display()))?;
        if !metadata.is_dir() {
            bail!("HERDR_PLUGIN_CONFIG_DIR must name a directory");
        }
        Ok(directory.join("config.toml"))
    }
}

impl PluginConfig {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&ConfigEnvironment::from_process())
    }

    fn load_from(environment: &ConfigEnvironment) -> anyhow::Result<Self> {
        let config_path = environment.config_path()?;
        let contents = fs::read_to_string(&config_path)
            .with_context(|| format!("read Activity config {}", config_path.display()))?;
        let raw: RawConfig = toml::from_str(&contents)
            .map_err(|mut error| {
                error.set_input(None);
                error
            })
            .with_context(|| format!("parse Activity config {}", config_path.display()))?;
        if raw.request_timeout_seconds == Some(0) {
            bail!("request_timeout_seconds must be greater than zero");
        }
        if raw.refresh_interval_seconds == 0 {
            bail!("refresh_interval_seconds must be greater than zero");
        }
        let auth = RuntimeAuth::resolve(environment)?;
        let api_base_url = validate_base_url(raw.api_base_url, auth.is_some())?;
        let timezone = resolve_timezone(raw.timezone.as_deref(), environment)?;
        Ok(Self {
            api_base_url,
            request_timeout: raw.request_timeout_seconds.map(Duration::from_secs),
            refresh_interval: Duration::from_secs(raw.refresh_interval_seconds),
            timezone,
            auth,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    api_base_url: Url,
    request_timeout_seconds: Option<u64>,
    refresh_interval_seconds: u64,
    timezone: Option<String>,
}

pub(crate) struct RuntimeAuth;

impl RuntimeAuth {
    pub(crate) fn resolve(environment: &ConfigEnvironment) -> anyhow::Result<Option<SecretString>> {
        let value = match (&environment.token, &environment.token_file) {
            (Some(_), Some(_)) => {
                bail!("AGENTSVIEW_TOKEN and AGENTSVIEW_TOKEN_FILE are both set")
            }
            (Some(token), None) => token
                .to_str()
                .context("AGENTSVIEW_TOKEN is not valid UTF-8")?
                .to_owned(),
            (None, Some(path)) => {
                let mut contents = fs::read_to_string(path)
                    .with_context(|| format!("read AGENTSVIEW_TOKEN_FILE {}", path.display()))?;
                if contents.ends_with("\r\n") {
                    contents.truncate(contents.len() - 2);
                } else if contents.ends_with('\n') {
                    contents.pop();
                }
                contents
            }
            (None, None) => return Ok(None),
        };
        validate_token(&value)?;
        Ok(Some(SecretString::from(value)))
    }
}

fn validate_token(value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        bail!("AgentsView bearer credential must not be empty");
    }
    if value.chars().any(char::is_control) {
        bail!("AgentsView bearer credential contains a control character");
    }
    Ok(())
}

pub(crate) fn validate_base_url(mut url: Url, has_auth: bool) -> anyhow::Result<Url> {
    if !url.username().is_empty() || url.password().is_some() {
        bail!("api_base_url must not contain user information");
    }
    if url.query().is_some() {
        bail!("api_base_url must not contain a query");
    }
    if url.fragment().is_some() {
        bail!("api_base_url must not contain a fragment");
    }
    match url.scheme() {
        "https" => {}
        "http" if has_auth => {
            bail!("bearer authentication requires an HTTPS api_base_url")
        }
        "http" if is_literal_loopback(&url) => {}
        "http" => bail!("plain HTTP api_base_url requires a literal loopback address"),
        scheme => bail!("unsupported api_base_url scheme {scheme:?}; use HTTPS"),
    }
    if url.cannot_be_a_base() || url.host().is_none() {
        bail!("api_base_url must be an absolute hierarchical URL with a host");
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn is_literal_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    }
}

fn resolve_timezone(
    configured: Option<&str>,
    environment: &ConfigEnvironment,
) -> anyhow::Result<Tz> {
    if let Some(value) = configured {
        return value
            .parse()
            .with_context(|| format!("invalid configured IANA timezone {value:?}"));
    }
    if let Some(value) = environment.tz.as_ref().and_then(|value| value.to_str()) {
        if let Ok(timezone) = value.parse() {
            return Ok(timezone);
        }
    }
    let value = environment
        .os_timezone
        .as_ref()
        .map_err(|error| anyhow::anyhow!("resolve operating-system timezone: {error}"))?;
    value
        .parse()
        .with_context(|| format!("invalid operating-system IANA timezone {value:?}"))
}

#[cfg(test)]
mod tests;
