// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::fmt;

use anyhow::Context;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::error::Category;
use serde_path_to_error::{Path, Segment};
use unicode_width::UnicodeWidthChar;
use url::Url;

use crate::config::{validate_base_url, PluginConfig};
use crate::wire::{
    AgentInfo, AgentsResponse, BranchInfo, BranchesResponse, MachinesResponse, ProjectInfo,
    ProjectsResponse, Report, ReportSelection, SessionPage, SessionRow, ACTIVITY_SCHEMA_VERSION,
};

const ERROR_EXCERPT_CHARS: usize = 160;
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_SUCCESS_BODY_BYTES: usize = 16 * 1024 * 1024;
const SESSION_PAGE_LIMIT: usize = 500;
const MAX_REPORT_GENERATION_RESTARTS: usize = 1;

#[derive(Clone, Debug, PartialEq)]
pub enum SessionFetch {
    Rows(Vec<SessionRow>),
    Refreshed(Box<Report>),
}

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
        self.hydrate_report(decode_report(&body)?).await
    }

    pub async fn fetch_bucket_sessions(
        &self,
        report_id: &str,
        bucket: usize,
    ) -> Result<SessionFetch, ApiError> {
        let first = self
            .fetch_session_page(report_id, None, Some(bucket))
            .await?;
        if first.refresh_required {
            return self.hydrate_replacement(first).await;
        }
        validate_page_report_id(&first, report_id)?;
        let total = first.total;
        let mut rows = first.sessions;
        let mut cursor = first.next_cursor;
        validate_cursor_progress(rows.len(), cursor.as_deref())?;
        validate_cursor_at_total(rows.len(), total, cursor.as_deref())?;
        let mut seen_cursors = BTreeSet::new();
        while let Some(current) = cursor {
            if !seen_cursors.insert(current.clone()) {
                return Err(ApiError::protocol(
                    "AgentsView repeated an Activity session cursor",
                ));
            }
            let page = self
                .fetch_session_page(report_id, Some(&current), None)
                .await?;
            if page.refresh_required {
                return self.hydrate_replacement(page).await;
            }
            validate_page_report_id(&page, report_id)?;
            if page.total != total {
                return Err(ApiError::protocol(
                    "AgentsView changed the Activity bucket total while paging",
                ));
            }
            validate_cursor_progress(page.sessions.len(), page.next_cursor.as_deref())?;
            append_session_page(&mut rows, page.sessions, total)?;
            validate_cursor_at_total(rows.len(), total, page.next_cursor.as_deref())?;
            cursor = page.next_cursor;
        }
        validate_session_rows(&rows, total)?;
        Ok(SessionFetch::Rows(rows))
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

    pub async fn fetch_branches(&self) -> Result<Vec<BranchInfo>, ApiError> {
        let body = self.get("api/v1/branches", &metadata_query()).await?;
        serde_json::from_slice::<BranchesResponse>(&body)
            .map(BranchesResponse::into_branches)
            .map_err(|_| ApiError::protocol("AgentsView returned invalid branches metadata"))
    }

    async fn get(&self, path: &str, query: &[(&'static str, String)]) -> Result<Vec<u8>, ApiError> {
        let endpoint = self
            .base_url
            .join(path)
            .map_err(|_| ApiError::protocol("invalid AgentsView endpoint configuration"))?;
        self.get_endpoint(endpoint, query).await
    }

    async fn fetch_session_page(
        &self,
        report_id: &str,
        cursor: Option<&str>,
        bucket: Option<usize>,
    ) -> Result<SessionPage, ApiError> {
        if report_id.is_empty() {
            return Err(ApiError::protocol(
                "AgentsView Activity report is missing a usable report_id",
            ));
        }
        let mut endpoint = self
            .base_url
            .join("api/v1/activity/report/")
            .map_err(|_| ApiError::protocol("invalid AgentsView endpoint configuration"))?;
        endpoint
            .path_segments_mut()
            .map_err(|_| ApiError::protocol("invalid AgentsView endpoint configuration"))?
            .pop_if_empty()
            .push(report_id)
            .push("sessions");
        let mut query = vec![("limit", SESSION_PAGE_LIMIT.to_string())];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor.to_owned()));
        } else if let Some(bucket) = bucket {
            query.push(("sort", "agent_minutes".to_owned()));
            query.push(("direction", "desc".to_owned()));
            query.push(("bucket", bucket.to_string()));
        }
        let body = self.get_endpoint(endpoint, &query).await?;
        decode_contract(&body, "schema v6 Activity session-page")
    }

    async fn hydrate_report(&self, mut report: Report) -> Result<Report, ApiError> {
        validate_report_contract(&report)?;
        for restart in 0..=MAX_REPORT_GENERATION_RESTARTS {
            match self.hydrate_report_generation(report).await? {
                Hydration::Complete(value) => return Ok(value),
                Hydration::Refreshed(value) if restart < MAX_REPORT_GENERATION_RESTARTS => {
                    report = value;
                }
                Hydration::Refreshed(_) => {
                    return Err(ApiError::new(
                        ApiErrorKind::Server,
                        "AgentsView Activity data changed repeatedly while paging; retry",
                    ));
                }
            }
        }
        unreachable!("bounded Activity report hydration loop")
    }

    async fn hydrate_report_generation(&self, mut report: Report) -> Result<Hydration, ApiError> {
        validate_report_contract(&report)?;
        if report.by_session.len() > report.sessions_total {
            return Err(ApiError::protocol(
                "AgentsView Activity report contains more sessions than sessions_total",
            ));
        }
        let Some(mut cursor) = report.sessions_next_cursor.take() else {
            validate_session_rows(&report.by_session, report.sessions_total)?;
            return Ok(Hydration::Complete(report));
        };
        validate_cursor_progress(report.by_session.len(), Some(&cursor))?;
        validate_cursor_at_total(
            report.by_session.len(),
            report.sessions_total,
            Some(&cursor),
        )?;
        let report_id = report.report_id.clone().ok_or_else(|| {
            ApiError::protocol("AgentsView paged Activity report is missing report_id")
        })?;
        let mut seen_cursors = BTreeSet::new();
        loop {
            if !seen_cursors.insert(cursor.clone()) {
                return Err(ApiError::protocol(
                    "AgentsView repeated an Activity session cursor",
                ));
            }
            let page = self
                .fetch_session_page(&report_id, Some(&cursor), None)
                .await?;
            if page.refresh_required {
                let replacement = replacement_report(page)?;
                validate_report_contract(&replacement)?;
                return Ok(Hydration::Refreshed(replacement));
            }
            validate_page_report_id(&page, &report_id)?;
            if page.total != report.sessions_total {
                return Err(ApiError::protocol(
                    "AgentsView changed sessions_total while paging Activity",
                ));
            }
            validate_cursor_progress(page.sessions.len(), page.next_cursor.as_deref())?;
            append_session_page(&mut report.by_session, page.sessions, report.sessions_total)?;
            validate_cursor_at_total(
                report.by_session.len(),
                report.sessions_total,
                page.next_cursor.as_deref(),
            )?;
            match page.next_cursor {
                Some(next) => cursor = next,
                None => break,
            }
        }
        validate_session_rows(&report.by_session, report.sessions_total)?;
        Ok(Hydration::Complete(report))
    }

    async fn hydrate_replacement(&self, page: SessionPage) -> Result<SessionFetch, ApiError> {
        let replacement = replacement_report(page)?;
        validate_report_contract(&replacement)?;
        self.hydrate_report(replacement)
            .await
            .map(Box::new)
            .map(SessionFetch::Refreshed)
    }

    async fn get_endpoint(
        &self,
        mut endpoint: Url,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, ApiError> {
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

enum Hydration {
    Complete(Report),
    Refreshed(Report),
}

fn decode_report(body: &[u8]) -> Result<Report, ApiError> {
    let version: VersionEnvelope = serde_json::from_slice(body).map_err(|error| {
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
    decode_contract(body, "schema v6")
}

fn decode_contract<T: DeserializeOwned>(body: &[u8], contract: &str) -> Result<T, ApiError> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = safe_contract_path(error.path());
        let message = format!("AgentsView response does not match the {contract} contract");
        if path.is_empty() {
            ApiError::protocol(message)
        } else {
            ApiError::protocol(format!("{message} at {path}"))
        }
    })
}

fn validate_report_contract(report: &Report) -> Result<(), ApiError> {
    if report.schema_version != ACTIVITY_SCHEMA_VERSION {
        return Err(ApiError::protocol(format!(
            "unsupported Activity schema version {}; expected {ACTIVITY_SCHEMA_VERSION}",
            report.schema_version
        )));
    }
    if report.totals.sessions != report.sessions_total {
        return Err(ApiError::protocol(
            "AgentsView Activity report has contradictory session totals",
        ));
    }
    Ok(())
}

fn validate_page_report_id(page: &SessionPage, expected: &str) -> Result<(), ApiError> {
    if page.report_id == expected {
        Ok(())
    } else {
        Err(ApiError::protocol(
            "AgentsView changed report_id while paging Activity",
        ))
    }
}

fn replacement_report(page: SessionPage) -> Result<Report, ApiError> {
    let report = page.report.ok_or_else(|| {
        ApiError::protocol("AgentsView requested an Activity refresh without a replacement report")
    })?;
    if report.report_id.as_deref() != Some(page.report_id.as_str()) {
        return Err(ApiError::protocol(
            "AgentsView Activity replacement report_id does not match the page",
        ));
    }
    Ok(*report)
}

fn append_session_page(
    rows: &mut Vec<SessionRow>,
    page: Vec<SessionRow>,
    total: usize,
) -> Result<(), ApiError> {
    if rows.len().saturating_add(page.len()) > total {
        return Err(ApiError::protocol(
            "AgentsView returned more Activity sessions than the page total",
        ));
    }
    rows.extend(page);
    Ok(())
}

fn validate_cursor_progress(row_count: usize, next_cursor: Option<&str>) -> Result<(), ApiError> {
    if row_count == 0 && next_cursor.is_some() {
        Err(ApiError::protocol(
            "AgentsView Activity session cursor did not advance",
        ))
    } else {
        Ok(())
    }
}

fn validate_cursor_at_total(
    row_count: usize,
    total: usize,
    next_cursor: Option<&str>,
) -> Result<(), ApiError> {
    if next_cursor.is_some() && row_count >= total {
        Err(ApiError::protocol(
            "AgentsView Activity report has a cursor after the final session",
        ))
    } else {
        Ok(())
    }
}

fn validate_session_rows(rows: &[SessionRow], expected: usize) -> Result<(), ApiError> {
    let actual = rows.len();
    if actual == expected {
        let unique = rows
            .iter()
            .map(|row| row.session_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        if unique == actual {
            Ok(())
        } else {
            Err(ApiError::protocol(
                "AgentsView repeated a session while paging Activity",
            ))
        }
    } else {
        Err(ApiError::protocol(format!(
            "AgentsView Activity paging returned {actual} sessions; expected {expected}"
        )))
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
    let mut redact_next_map_key = false;
    for segment in path {
        let redact_this_map_key = std::mem::take(&mut redact_next_map_key);
        match segment {
            Segment::Seq { index } => rendered.push_str(&format!("[{index}]")),
            Segment::Map { key } => {
                if redact_this_map_key {
                    rendered.push_str("[*]");
                    continue;
                }
                if !rendered.is_empty() {
                    rendered.push('.');
                }
                rendered.push_str(&safe_excerpt(key.as_bytes()));
                // Keep this list aligned with dynamic-key maps in the wire contract.
                redact_next_map_key = matches!(key.as_str(), "models" | "projects");
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
