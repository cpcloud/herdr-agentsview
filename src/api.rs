use std::fmt;

use anyhow::Context;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::error::Category;
use serde_path_to_error::{Path, Segment};
use unicode_width::UnicodeWidthChar;
use url::Url;

use crate::config::{validate_base_url, PluginConfig};
use crate::wire::{
    AgentInfo, AgentsResponse, MachinesResponse, ProjectInfo, ProjectsResponse, Report,
    ReportSelection, ACTIVITY_SCHEMA_VERSION,
};

const ERROR_EXCERPT_CHARS: usize = 160;
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_SUCCESS_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct ActivityClient {
    http: Client,
    base_url: Url,
    auth: Option<SecretString>,
}

impl ActivityClient {
    pub fn new(config: &PluginConfig) -> anyhow::Result<Self> {
        Self::build(config, None)
    }

    #[cfg(test)]
    fn new_with_root(config: &PluginConfig, root: reqwest::Certificate) -> anyhow::Result<Self> {
        Self::build(config, Some(root))
    }

    fn build(
        config: &PluginConfig,
        test_root: Option<reqwest::Certificate>,
    ) -> anyhow::Result<Self> {
        let base_url = validate_base_url(config.api_base_url.clone(), config.auth.is_some())?;
        let mut builder = Client::builder().redirect(Policy::none());
        if let Some(timeout) = config.request_timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(root) = test_root {
            builder = builder.add_root_certificate(root);
        }
        let http = builder.build().context("build AgentsView HTTP client")?;
        Ok(Self {
            http,
            base_url,
            auth: config.auth.clone(),
        })
    }

    pub async fn fetch_report(&self, selection: &ReportSelection) -> Result<Report, ApiError> {
        let body = self
            .get("api/v1/activity/report", &selection.query_pairs())
            .await?;
        let version: VersionEnvelope = serde_json::from_slice(&body).map_err(|error| {
            if matches!(error.classify(), Category::Syntax | Category::Eof) {
                ApiError::protocol("AgentsView returned invalid JSON for Activity")
            } else {
                ApiError::protocol(
                    "AgentsView Activity response has an incompatible schema_version type",
                )
            }
        })?;
        let version = version.schema_version.ok_or_else(|| {
            ApiError::protocol("AgentsView Activity response is missing schema_version")
        })?;
        if version != ACTIVITY_SCHEMA_VERSION {
            return Err(ApiError::protocol(format!(
                "unsupported Activity schema version {version}; expected {ACTIVITY_SCHEMA_VERSION}"
            )));
        }
        let mut deserializer = serde_json::Deserializer::from_slice(&body);
        serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
            let path = safe_contract_path(error.path());
            let message = "AgentsView response does not match the schema v5 contract";
            if path.is_empty() {
                ApiError::protocol(message)
            } else {
                ApiError::protocol(format!("{message} at {path}"))
            }
        })
    }

    pub async fn fetch_projects(&self) -> Result<Vec<ProjectInfo>, ApiError> {
        let body = self.get("api/v1/projects", &metadata_query()).await?;
        serde_json::from_slice::<ProjectsResponse>(&body)
            .map(ProjectsResponse::into_projects)
            .map_err(|_| ApiError::protocol("AgentsView returned invalid projects metadata"))
    }

    pub async fn fetch_agents(&self) -> Result<Vec<AgentInfo>, ApiError> {
        let body = self.get("api/v1/agents", &metadata_query()).await?;
        serde_json::from_slice::<AgentsResponse>(&body)
            .map(AgentsResponse::into_agents)
            .map_err(|_| ApiError::protocol("AgentsView returned invalid agents metadata"))
    }

    pub async fn fetch_machines(&self) -> Result<Vec<String>, ApiError> {
        let body = self.get("api/v1/machines", &metadata_query()).await?;
        serde_json::from_slice::<MachinesResponse>(&body)
            .map(MachinesResponse::into_machines)
            .map_err(|_| ApiError::protocol("AgentsView returned invalid machines metadata"))
    }

    async fn get(&self, path: &str, query: &[(&'static str, String)]) -> Result<Vec<u8>, ApiError> {
        let mut endpoint = self
            .base_url
            .join(path)
            .map_err(|_| ApiError::protocol("invalid AgentsView endpoint configuration"))?;
        {
            let mut pairs = endpoint.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        let mut request = self.http.get(endpoint);
        if let Some(auth) = &self.auth {
            request = request.bearer_auth(auth.expose_secret());
        }
        let response = request.send().await.map_err(ApiError::from_reqwest)?;
        let status = response.status();
        let authenticated = self.auth.is_some();
        if !status.is_success()
            && (authenticated
                || status == StatusCode::UNAUTHORIZED
                || status == StatusCode::FORBIDDEN
                || status.is_redirection())
        {
            return Err(ApiError::from_status(status, &[], authenticated));
        }
        let body_limit = if status.is_success() {
            MAX_SUCCESS_BODY_BYTES
        } else {
            MAX_ERROR_BODY_BYTES
        };
        if response
            .content_length()
            .is_some_and(|length| length > body_limit as u64)
        {
            return if status.is_success() {
                Err(ApiError::protocol(
                    "AgentsView response body is too large for the Activity dashboard",
                ))
            } else {
                Err(ApiError::from_status(status, &[], authenticated))
            };
        }
        let body = read_bounded_body(response, body_limit).await?;
        if body.truncated && status.is_success() {
            return Err(ApiError::protocol(
                "AgentsView response body is too large for the Activity dashboard",
            ));
        }
        if !status.is_success() {
            return Err(ApiError::from_status(status, &body.bytes, authenticated));
        }
        Ok(body.bytes)
    }
}

struct BoundedBody {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded_body(mut response: Response, limit: usize) -> Result<BoundedBody, ApiError> {
    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(limit);
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await.map_err(ApiError::from_reqwest)? {
        let remaining = limit - bytes.len();
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            return Ok(BoundedBody {
                bytes,
                truncated: true,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(BoundedBody {
        bytes,
        truncated: false,
    })
}

fn metadata_query() -> Vec<(&'static str, String)> {
    vec![
        ("include_one_shot", "true".to_owned()),
        ("include_automated", "true".to_owned()),
    ]
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiErrorKind {
    Authentication,
    Forbidden,
    Timeout,
    Network,
    Protocol,
    Server,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub message: String,
}

impl ApiError {
    pub fn timeout() -> Self {
        Self::new(
            ApiErrorKind::Timeout,
            "AgentsView request timed out; retry or increase the request timeout",
        )
    }

    fn server_timeout() -> Self {
        Self::new(
            ApiErrorKind::Timeout,
            "AgentsView stopped the Activity report at its server timeout",
        )
    }

    fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn protocol(message: impl Into<String>) -> Self {
        Self::new(ApiErrorKind::Protocol, message)
    }

    fn from_reqwest(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::timeout();
        }
        if error.is_body() {
            return Self::new(
                ApiErrorKind::Network,
                "AgentsView response ended before the complete body arrived",
            );
        }
        Self::new(
            ApiErrorKind::Network,
            "could not reach AgentsView; check the API URL and network",
        )
    }

    fn from_status(status: StatusCode, body: &[u8], authenticated: bool) -> Self {
        if status == StatusCode::SERVICE_UNAVAILABLE
            && !authenticated
            && is_agentsview_write_timeout(body)
        {
            return Self::server_timeout();
        }
        match status {
            StatusCode::UNAUTHORIZED if authenticated => Self::new(
                ApiErrorKind::Authentication,
                "AgentsView rejected the configured credential (HTTP 401); check the runtime token",
            ),
            StatusCode::UNAUTHORIZED => Self::new(
                ApiErrorKind::Authentication,
                "AgentsView requires authentication (HTTP 401); configure a runtime token",
            ),
            StatusCode::FORBIDDEN => Self::new(
                ApiErrorKind::Forbidden,
                "AgentsView access is forbidden (HTTP 403); check server authorization policy",
            ),
            status if status.is_redirection() => Self::new(
                ApiErrorKind::Protocol,
                format!(
                    "AgentsView returned a redirect (HTTP {}); redirects are disabled",
                    status.as_u16()
                ),
            ),
            status if status.is_server_error() => Self::new(
                ApiErrorKind::Server,
                status_message("AgentsView server returned", status, body, authenticated),
            ),
            status => Self::new(
                ApiErrorKind::Protocol,
                status_message(
                    "AgentsView request failed with",
                    status,
                    body,
                    authenticated,
                ),
            ),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Deserialize)]
struct VersionEnvelope {
    schema_version: Option<u32>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: Option<String>,
}

fn is_agentsview_write_timeout(body: &[u8]) -> bool {
    serde_json::from_slice::<ErrorEnvelope>(body)
        .is_ok_and(|envelope| envelope.error.as_deref() == Some("request timed out"))
}

fn status_message(prefix: &str, status: StatusCode, body: &[u8], authenticated: bool) -> String {
    let base = format!("{prefix} HTTP {}", status.as_u16());
    if authenticated {
        return base;
    }
    let excerpt = safe_excerpt(body);
    if excerpt.is_empty() {
        base
    } else {
        format!("{base}: {excerpt}")
    }
}

fn safe_excerpt(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut excerpt = String::new();
    let mut character_count = 0;
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = !excerpt.is_empty();
            continue;
        }
        if character.is_control() || UnicodeWidthChar::width(character).unwrap_or(0) == 0 {
            continue;
        }
        if pending_space {
            if character_count + 1 >= ERROR_EXCERPT_CHARS {
                break;
            }
            excerpt.push(' ');
            character_count += 1;
            pending_space = false;
        }
        if character_count == ERROR_EXCERPT_CHARS {
            break;
        }
        excerpt.push(character);
        character_count += 1;
    }
    excerpt
}

fn safe_contract_path(path: &Path) -> String {
    let mut rendered = String::new();
    let mut redact_map_key = false;
    for segment in path {
        match segment {
            Segment::Seq { index } => rendered.push_str(&format!("[{index}]")),
            Segment::Map { key } => {
                if redact_map_key {
                    rendered.push_str("[*]");
                    redact_map_key = false;
                    continue;
                }
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(&safe_excerpt(key.as_bytes()));
                // Keep this list aligned with dynamic-key maps in the wire contract.
                redact_map_key = matches!(key.as_str(), "models" | "projects");
            }
            Segment::Enum { variant } => {
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(&safe_excerpt(variant.as_bytes()));
            }
            Segment::Unknown => {
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push('?');
            }
        }
    }
    safe_excerpt(rendered.as_bytes())
}

#[cfg(test)]
mod tests;
