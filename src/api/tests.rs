// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, SystemTime};

use chrono::NaiveDate;
use chrono_tz::Tz;
use secrecy::SecretString;
use tokio::net::TcpListener;
use url::Url;

use crate::config::PluginConfig;
use crate::wire::{Automation, ReportSelection};

use super::{
    safe_contract_path, safe_excerpt, ActivityClient, ApiErrorKind, SessionFetch,
    MAX_ERROR_BODY_BYTES, MAX_SUCCESS_BODY_BYTES,
};

mod http;

use http::{RecordingServer, ResponsePlan};

const REPORT_FIXTURE: &str = include_str!("../../tests/fixtures/report-v6.json");
const PROJECTS_FIXTURE: &str = include_str!("../../tests/fixtures/projects.json");
const AGENTS_FIXTURE: &str = include_str!("../../tests/fixtures/agents.json");
const MACHINES_FIXTURE: &str = include_str!("../../tests/fixtures/machines.json");

fn selection() -> ReportSelection {
    ReportSelection::new(
        NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
        "America/New_York".parse().unwrap(),
    )
}

fn config(base_url: Url, auth: Option<SecretString>, timeout: Duration) -> PluginConfig {
    PluginConfig {
        api_base_url: base_url,
        request_timeout: Some(timeout),
        refresh_interval: Duration::from_secs(300),
        timezone: Tz::UTC,
        auth,
    }
}

fn generated_credential() -> String {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}-{nonce:x}", std::process::id())
}

#[tokio::test]
async fn report_request_uses_only_supported_activity_query() {
    // If request construction adds unsupported selectors or drops an approved filter,
    // the official Activity endpoint can reject the request or return the wrong report.
    let mut server = RecordingServer::start(ResponsePlan::json(REPORT_FIXTURE)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();
    let selection = selection()
        .with_project("project-alpha")
        .with_agent("codex")
        .with_machine("machine-alpha")
        .with_automation(Automation::Automated);

    let report = client.fetch_report(&selection).await.unwrap();
    let request = server.take_request().await;

    assert_eq!(report.totals.sessions, 3);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/api/v1/activity/report");
    assert_eq!(
        request.query,
        vec![
            ("preset".to_owned(), "day".to_owned()),
            ("date".to_owned(), "2026-08-08".to_owned()),
            ("timezone".to_owned(), "America/New_York".to_owned()),
            ("project".to_owned(), "project-alpha".to_owned()),
            ("agent".to_owned(), "codex".to_owned()),
            ("machine".to_owned(), "machine-alpha".to_owned()),
            ("automation".to_owned(), "automated".to_owned()),
        ]
    );
    assert!(!request.had_bearer_header);
}

#[tokio::test]
async fn metadata_requests_include_one_shot_and_automated_sessions() {
    // If either inclusion flag disappears, selector contents no longer describe the same
    // population as the Activity report, which always includes one-shot sessions.
    let expected_query = vec![
        ("include_one_shot".to_owned(), "true".to_owned()),
        ("include_automated".to_owned(), "true".to_owned()),
    ];

    let mut projects = RecordingServer::start(ResponsePlan::json(PROJECTS_FIXTURE)).await;
    let client = ActivityClient::new(&config(
        projects.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();
    assert!(client.fetch_projects().await.unwrap().is_empty());
    let request = projects.take_request().await;
    assert_eq!(request.path, "/api/v1/projects");
    assert_eq!(request.query, expected_query);

    let mut agents = RecordingServer::start(ResponsePlan::json(AGENTS_FIXTURE)).await;
    let client = ActivityClient::new(&config(
        agents.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();
    assert!(client.fetch_agents().await.unwrap().is_empty());
    let request = agents.take_request().await;
    assert_eq!(request.path, "/api/v1/agents");
    assert_eq!(request.query, expected_query);

    let mut machines = RecordingServer::start(ResponsePlan::json(MACHINES_FIXTURE)).await;
    let client = ActivityClient::new(&config(
        machines.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();
    assert!(client.fetch_machines().await.unwrap().is_empty());
    let request = machines.take_request().await;
    assert_eq!(request.path, "/api/v1/machines");
    assert_eq!(request.query, expected_query);
}

#[tokio::test]
async fn bearer_authentication_is_sent_only_over_verified_tls() {
    // If the client omits the bearer header or weakens TLS verification, authenticated
    // deployments either fail closed or disclose runtime credentials.
    let credential = generated_credential();
    let (mut server, root) = RecordingServer::start_tls(
        ResponsePlan::json(REPORT_FIXTURE),
        SecretString::from(credential.clone()),
    )
    .await;
    let config = config(
        server.base_url().clone(),
        Some(SecretString::from(credential)),
        Duration::from_secs(2),
    );
    let client = ActivityClient::new_with_root(&config, root).unwrap();

    client.fetch_report(&selection()).await.unwrap();
    let request = server.take_request().await;

    assert!(request.had_bearer_header);
    assert_eq!(request.bearer_matched, Some(true));
}

#[tokio::test]
async fn unauthorized_without_credentials_has_an_authentication_hint() {
    // If an unauthenticated 401 is reported as a generic network failure, the operator
    // cannot distinguish missing configuration from an unreachable server.
    let mut server = RecordingServer::start(ResponsePlan::status_then_wait(401)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Authentication);
    assert!(error.to_string().contains("requires authentication"));
    server.take_request().await;
    tokio::time::timeout(Duration::from_secs(2), server.finish())
        .await
        .expect("classified unauthenticated response must release its socket")
        .unwrap();
}

#[tokio::test]
async fn credentialed_error_never_retains_a_reflected_secret() {
    // If an authenticated error includes the response body, a malicious or misconfigured
    // server can reflect the bearer value into persistent terminal output.
    let credential = generated_credential();
    let reflected = format!("rejected credential: {credential}");
    let (server, root) = RecordingServer::start_tls(
        ResponsePlan::status(401, reflected.as_bytes()),
        SecretString::from(credential.clone()),
    )
    .await;
    let config = config(
        server.base_url().clone(),
        Some(SecretString::from(credential.clone())),
        Duration::from_secs(2),
    );
    let client = ActivityClient::new_with_root(&config, root).unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();
    let rendered = error.to_string();

    assert_eq!(error.kind, ApiErrorKind::Authentication);
    assert!(!rendered.contains(&credential));
    assert!(!rendered.contains("rejected credential"));
}

#[tokio::test]
async fn credentialed_server_error_never_retains_a_reflected_secret() {
    // If authenticated 5xx handling starts reading response bodies, a reflecting server
    // can copy the runtime credential into a stale-data banner.
    let credential = generated_credential();
    let reflected = format!("server failed while handling credential: {credential}");
    let (server, root) = RecordingServer::start_tls(
        ResponsePlan::status(500, reflected.as_bytes()),
        SecretString::from(credential.clone()),
    )
    .await;
    let config = config(
        server.base_url().clone(),
        Some(SecretString::from(credential.clone())),
        Duration::from_secs(2),
    );
    let client = ActivityClient::new_with_root(&config, root).unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();
    let rendered = error.to_string();

    assert_eq!(error.kind, ApiErrorKind::Server);
    assert!(!rendered.contains(&credential));
    assert!(!rendered.contains("server failed"));
}

#[tokio::test]
async fn credentialed_status_is_classified_without_waiting_for_its_body() {
    // If an authenticated error body is read before status classification, a server can
    // retain the runtime credential in client memory or turn an immediate 401 into a
    // misleading timeout by never completing the body.
    let credential = generated_credential();
    let (mut server, root) = RecordingServer::start_tls(
        ResponsePlan::status_then_wait(401),
        SecretString::from(credential.clone()),
    )
    .await;
    let config = config(
        server.base_url().clone(),
        Some(SecretString::from(credential)),
        Duration::from_secs(10),
    );
    let client = ActivityClient::new_with_root(&config, root).unwrap();

    let error = tokio::time::timeout(Duration::from_secs(2), client.fetch_report(&selection()))
        .await
        .expect("status classification must not wait for the response body")
        .unwrap_err();
    let request = server.take_request().await;

    assert_eq!(error.kind, ApiErrorKind::Authentication);
    assert_eq!(request.bearer_matched, Some(true));
    tokio::time::timeout(Duration::from_secs(2), server.finish())
        .await
        .expect("classified authenticated response must release its socket")
        .unwrap();
}

#[tokio::test]
async fn forbidden_response_is_distinct_from_authentication_failure() {
    // If 403 collapses into 401, recovery guidance can tell the operator to replace a
    // valid credential when the actual problem is authorization policy.
    let server = RecordingServer::start(ResponsePlan::status(403, b"forbidden")).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Forbidden);
    assert!(error.to_string().contains("forbidden"));
}

#[tokio::test]
async fn agentsview_write_timeout_is_classified_without_dumping_its_json_body() {
    // If the server's canonical write-timeout response stays a generic 503, the terminal
    // dumps clipped JSON instead of naming the boundary that stopped the Activity report.
    let body = br#"{"error":"request timed out","detail":"GET /api/v1/activity/report did not finish writing a response within the 30s write timeout"}"#;
    let server = RecordingServer::start(ResponsePlan::status(503, body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Timeout);
    assert_eq!(
        error.message,
        "AgentsView stopped the Activity report at its server timeout"
    );
    assert!(!error.message.contains('{'));
}

#[tokio::test]
async fn delayed_response_respects_the_configured_timeout() {
    // If request timeout wiring is dropped, a dead server can freeze the whole dashboard.
    let server =
        RecordingServer::start(ResponsePlan::json(REPORT_FIXTURE).delayed(Duration::from_secs(5)))
            .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_millis(100),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Timeout);
}

#[tokio::test]
async fn delayed_response_has_no_deadline_without_a_configured_timeout() {
    // If the HTTP client invents a deadline when none is configured, slow Activity
    // aggregation diverges from the AgentsView UI and fails before the server responds.
    let server = RecordingServer::start(
        ResponsePlan::json(REPORT_FIXTURE).delayed(Duration::from_millis(200)),
    )
    .await;
    let config = PluginConfig {
        api_base_url: server.base_url().clone(),
        request_timeout: None,
        refresh_interval: Duration::from_secs(300),
        timezone: Tz::UTC,
        auth: None,
    };
    let client = ActivityClient::new(&config).unwrap();

    let report = tokio::time::timeout(Duration::from_secs(2), client.fetch_report(&selection()))
        .await
        .expect("the delayed fixture must eventually respond")
        .unwrap();

    assert_eq!(report.schema_version, crate::wire::ACTIVITY_SCHEMA_VERSION);
}

#[tokio::test]
async fn connect_failure_is_classified_as_network() {
    // If connection failures are mislabeled as protocol errors, retry and stale-data
    // guidance cannot distinguish transport recovery from an incompatible server.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let base_url = Url::parse(&format!(
        "http://{}/",
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
    ))
    .unwrap();
    let client = ActivityClient::new(&config(base_url, None, Duration::from_secs(1))).unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Network);
}

#[tokio::test]
async fn malformed_json_is_a_protocol_error() {
    // If malformed JSON is treated as empty data, transport corruption can masquerade as
    // an inactive day.
    let server = RecordingServer::start(ResponsePlan::json(b"{".as_slice())).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("invalid JSON"));
}

#[tokio::test]
async fn current_v6_report_reaches_the_client() {
    // If standalone AgentsView advances its Activity contract while this strict client stays
    // stale, opening the dashboard fails before any report data reaches the app.
    let server = RecordingServer::start(ResponsePlan::json(REPORT_FIXTURE)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let report = client.fetch_report(&selection()).await.unwrap();

    assert_eq!(report.schema_version, 6);
    assert_eq!(report.by_session[0].session_id, "session-alpha");
}

#[tokio::test]
async fn current_v6_report_hydrates_every_session_page_for_local_sorts() {
    // If the v6 cursor is ignored, title and model sorts operate on only the bounded first
    // page even though the dashboard presents them as full-report sorts.
    let mut first: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let remaining = first["by_session"].as_array_mut().unwrap().split_off(1);
    first["sessions_next_cursor"] = serde_json::json!("fixture-cursor");
    let continuation = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": remaining,
        "total": 3
    });
    let mut server = RecordingServer::start_sequence(vec![
        ResponsePlan::json(serde_json::to_vec(&first).unwrap()),
        ResponsePlan::json(serde_json::to_vec(&continuation).unwrap()),
    ])
    .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let report = client.fetch_report(&selection()).await.unwrap();
    let report_request = server.take_request().await;
    let page_request = server.take_request().await;

    assert_eq!(report.by_session.len(), report.sessions_total);
    assert_eq!(report.by_session[2].session_id, "session-gamma");
    assert_eq!(report_request.path, "/api/v1/activity/report");
    assert_eq!(
        page_request.path,
        "/api/v1/activity/report/fixture-report-id/sessions"
    );
    assert_eq!(
        page_request.query,
        vec![
            ("limit".to_owned(), "500".to_owned()),
            ("cursor".to_owned(), "fixture-cursor".to_owned()),
        ]
    );
}

#[tokio::test]
async fn report_rejects_disagreeing_v6_session_totals() {
    // If summary and paging totals can diverge, the header and session table describe
    // different populations even after every declared page has been loaded.
    let mut report: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    report["totals"]["sessions"] = serde_json::json!(4);
    let server =
        RecordingServer::start(ResponsePlan::json(serde_json::to_vec(&report).unwrap())).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("session totals"));
}

#[tokio::test]
async fn report_rejects_a_cursor_after_the_declared_final_row() {
    // If a continuation cursor survives after the accumulated row count reaches total, an
    // impossible extra page can be requested and a malformed transcript accepted as complete.
    let mut first: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let remaining = first["by_session"].as_array_mut().unwrap().split_off(1);
    first["sessions_next_cursor"] = serde_json::json!("first-cursor");
    let final_rows = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": remaining,
        "next_cursor": "cursor-after-final-row",
        "total": 3
    });
    let empty_terminal_page = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": [],
        "total": 3
    });
    let server = RecordingServer::start_sequence(vec![
        ResponsePlan::json(serde_json::to_vec(&first).unwrap()),
        ResponsePlan::json(serde_json::to_vec(&final_rows).unwrap()),
        ResponsePlan::json(serde_json::to_vec(&empty_terminal_page).unwrap()),
    ])
    .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("cursor after the final session"));
}

#[tokio::test]
async fn bucket_sessions_use_the_exact_server_page_and_bucket_index() {
    // If the plugin approximates bucket membership from first/last activity, sessions with an
    // idle gap appear active even when AgentsView omits them from the exact membership page.
    let fixture: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let page = serde_json::json!({
        "report_id": "fixture/report id",
        "sessions": [fixture["by_session"][0].clone()],
        "total": 1
    });
    let mut server =
        RecordingServer::start(ResponsePlan::json(serde_json::to_vec(&page).unwrap())).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let result = client
        .fetch_bucket_sessions("fixture/report id", 7)
        .await
        .unwrap();
    let request = server.take_request().await;

    let SessionFetch::Rows(rows) = result else {
        panic!("stable bucket page must return rows");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "session-alpha");
    assert_eq!(
        request.path,
        "/api/v1/activity/report/fixture%2Freport%20id/sessions"
    );
    assert_eq!(
        request.query,
        vec![
            ("limit".to_owned(), "500".to_owned()),
            ("sort".to_owned(), "agent_minutes".to_owned()),
            ("direction".to_owned(), "desc".to_owned()),
            ("bucket".to_owned(), "7".to_owned()),
        ]
    );
}

#[tokio::test]
async fn bucket_sessions_reject_a_cursor_after_the_declared_final_row() {
    // If bucket paging loses its final-cursor check, an impossible extra page can be requested
    // and accepted even though exact timeline membership already reached the declared total.
    let fixture: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let first = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": [fixture["by_session"][0].clone()],
        "next_cursor": "first-bucket-cursor",
        "total": 2
    });
    let final_rows = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": [fixture["by_session"][1].clone()],
        "next_cursor": "cursor-after-final-bucket-row",
        "total": 2
    });
    let empty_terminal_page = serde_json::json!({
        "report_id": "fixture-report-id",
        "sessions": [],
        "total": 2
    });
    let server = RecordingServer::start_sequence(vec![
        ResponsePlan::json(serde_json::to_vec(&first).unwrap()),
        ResponsePlan::json(serde_json::to_vec(&final_rows).unwrap()),
        ResponsePlan::json(serde_json::to_vec(&empty_terminal_page).unwrap()),
    ])
    .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client
        .fetch_bucket_sessions("fixture-report-id", 0)
        .await
        .unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("cursor after the final session"));
}

#[tokio::test]
async fn bucket_refresh_replaces_the_report_generation_atomically() {
    // If refresh rows are appended to the old generation, totals and session membership can
    // combine two different source snapshots under one report ID.
    let mut replacement: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    replacement["report_id"] = serde_json::json!("replacement-report-id");
    let page = serde_json::json!({
        "report_id": "replacement-report-id",
        "sessions": replacement["by_session"].clone(),
        "total": replacement["sessions_total"].clone(),
        "refresh_required": true,
        "report": replacement
    });
    let server =
        RecordingServer::start(ResponsePlan::json(serde_json::to_vec(&page).unwrap())).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let result = client
        .fetch_bucket_sessions("old-report-id", 0)
        .await
        .unwrap();

    let SessionFetch::Refreshed(report) = result else {
        panic!("changed generation must replace the report");
    };
    assert_eq!(report.report_id.as_deref(), Some("replacement-report-id"));
    assert_eq!(report.by_session.len(), report.sessions_total);
}

#[tokio::test]
async fn report_schema_version_is_required_and_exact() {
    // If version preflight is skipped, incompatible fields can be reported only as vague
    // deserialization failures without identifying the server/client mismatch.
    for (body, expected) in [
        (r#"{}"#, "missing schema_version"),
        (
            r#"{"schema_version":5}"#,
            "unsupported Activity schema version 5",
        ),
        (
            r#"{"schema_version":7}"#,
            "unsupported Activity schema version 7",
        ),
    ] {
        let server = RecordingServer::start(ResponsePlan::json(body)).await;
        let client = ActivityClient::new(&config(
            server.base_url().clone(),
            None,
            Duration::from_secs(2),
        ))
        .unwrap();

        let error = client.fetch_report(&selection()).await.unwrap_err();

        assert_eq!(error.kind, ApiErrorKind::Protocol);
        assert!(error.to_string().contains(expected));
    }
}

#[tokio::test]
async fn well_formed_json_with_an_invalid_version_type_is_a_contract_error() {
    // If a valid JSON response with the wrong envelope type is called malformed JSON,
    // operators cannot distinguish transport corruption from an incompatible API.
    let server = RecordingServer::start(ResponsePlan::json(r#"{"schema_version":"6"}"#)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("schema_version type"));
    assert!(!error.to_string().contains("invalid JSON"));
}

#[tokio::test]
async fn same_version_unknown_field_is_a_contract_error() {
    // If strict schema-v6 decoding is bypassed after version preflight, new fields are
    // silently discarded and the UI can misrepresent server-computed totals.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    value["unexpected"] = serde_json::json!(true);
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("schema v6 contract"));
}

#[tokio::test]
async fn same_version_nested_shape_error_identifies_the_contract_path() {
    // If a nested field changes under schema v6, a generic mismatch leaves operators unable
    // to distinguish an upstream contract change from a stale fixture without exposing data.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    value["pricing"]["models"]["model-alpha"]["resolutions"][0]["bands"] =
        serde_json::json!("not-a-list");
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();
    let rendered = error.to_string();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(rendered.contains("pricing.models[*].resolutions[0].bands"));
    assert!(!rendered.contains("model-alpha"));
    assert!(!rendered.contains("not-a-list"));
}

#[test]
fn contract_path_redaction_does_not_cross_sequence_boundaries() {
    // If the dynamic-map marker survives a sequence segment, a later ordinary field is
    // needlessly redacted and the schema mismatch loses its useful location.
    let body = br#"{"models":[{"field":"not-a-boolean"}]}"#;
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let error =
        serde_path_to_error::deserialize::<_, BTreeMap<String, Vec<BTreeMap<String, bool>>>>(
            &mut deserializer,
        )
        .unwrap_err();

    assert_eq!(safe_contract_path(error.path()), "models[0].field");
}

#[tokio::test]
async fn contract_path_is_bounded_and_control_free_for_server_chosen_map_keys() {
    // If a server-chosen model key reaches the terminal through a contract path, it must not
    // inject controls or grow the retained error banner without bound.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let models = value["pricing"]["models"].as_object_mut().unwrap();
    let mut model = models.remove("model-alpha").unwrap();
    model["resolutions"][0]["bands"] = serde_json::json!("not-a-list");
    let hostile_key = format!("model\u{1b}]0;changed\u{7}{}", "x".repeat(500));
    models.insert(hostile_key, model);
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let rendered = client
        .fetch_report(&selection())
        .await
        .unwrap_err()
        .to_string();

    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{7}'));
    assert!(!rendered.contains("changed"));
    assert!(rendered.chars().count() <= 240);
}

#[tokio::test]
async fn contract_path_sanitizes_hostile_unknown_field_names() {
    // If an unknown field name contains terminal controls, the protocol diagnostic must
    // remain safe and bounded even though that path segment is not a redacted map key.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    let hostile_key = format!("unexpected\u{1b}]0;changed\u{7}{}", "x".repeat(500));
    value[hostile_key] = serde_json::json!(true);
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let rendered = client
        .fetch_report(&selection())
        .await
        .unwrap_err()
        .to_string();

    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{7}'));
    assert!(rendered.contains("unexpected"));
    assert!(rendered.chars().count() <= 240);
}

#[tokio::test]
async fn contract_path_redacts_project_map_keys() {
    // If project-map decoding fails, the useful field path must not disclose the
    // server-provided project identity retained in the response.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    value["projects"]["pl1-alpha"]["display_label"] = serde_json::json!(false);
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let rendered = client
        .fetch_report(&selection())
        .await
        .unwrap_err()
        .to_string();

    assert!(rendered.contains("projects[*].display_label"));
    assert!(!rendered.contains("pl1-alpha"));
}

#[tokio::test]
async fn unprintable_contract_path_falls_back_to_the_generic_message() {
    // If every path character is stripped for terminal safety, the diagnostic must remain
    // a complete sentence instead of ending with a dangling location preposition.
    let mut value: serde_json::Value = serde_json::from_str(REPORT_FIXTURE).unwrap();
    value["\u{1b}\u{7}"] = serde_json::json!(true);
    let body = serde_json::to_vec(&value).unwrap();
    let server = RecordingServer::start(ResponsePlan::json(body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let rendered = client
        .fetch_report(&selection())
        .await
        .unwrap_err()
        .to_string();

    assert_eq!(
        rendered,
        "AgentsView response does not match the schema v6 contract"
    );
}

#[tokio::test]
async fn unauthenticated_status_excerpt_is_bounded_and_control_free() {
    // If arbitrary response bodies flow into errors, a failing server can corrupt the
    // terminal or flood the retained stale-data banner.
    let body = format!("server\nproblem\u{1b}[31m{}", "x".repeat(500));
    let server = RecordingServer::start(ResponsePlan::status(500, body.as_bytes())).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();
    let rendered = error.to_string();

    assert_eq!(error.kind, ApiErrorKind::Server);
    assert!(rendered.contains("server problem"));
    assert!(!rendered.contains('\n'));
    assert!(!rendered.contains('\u{1b}'));
    assert!(rendered.len() < 260);
}

#[test]
fn status_excerpt_removes_invisible_formatting_within_its_character_budget() {
    // If zero-width or bidirectional formatting survives sanitization, an untrusted
    // server error can reorder or conceal the terminal's diagnostic text.
    let body = format!(
        "safe\u{202e}reversed\u{2066}isolated\u{200b}hidden {}",
        "x".repeat(500)
    );

    let excerpt = safe_excerpt(body.as_bytes());

    assert!(!excerpt.contains('\u{202e}'));
    assert!(!excerpt.contains('\u{2066}'));
    assert!(!excerpt.contains('\u{200b}'));
    assert!(excerpt.chars().count() <= 160);
}

#[test]
fn status_excerpt_never_spends_its_last_cell_on_trailing_space() {
    // If separator insertion consumes the final budget cell without its following word,
    // an error banner ends in misleading blank padding.
    let body = format!("{} next", "x".repeat(159));

    let excerpt = safe_excerpt(body.as_bytes());

    assert_eq!(excerpt.trim_end(), excerpt);
    assert!(excerpt.chars().count() <= 160);
}

#[tokio::test]
async fn oversized_success_stream_without_content_length_is_bounded() {
    // If streaming limits rely only on Content-Length, a close-delimited response can
    // restore unbounded buffering despite the declared-length preflight.
    let body = vec![b'x'; MAX_SUCCESS_BODY_BYTES + 1];
    let server = RecordingServer::start(ResponsePlan::without_content_length(200, body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(5),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("body is too large"));
}

#[tokio::test]
async fn oversized_error_stream_without_content_length_keeps_a_bounded_excerpt() {
    // If close-delimited error streams bypass the body cap, classification can retain an
    // arbitrarily large hostile response before the visible excerpt is shortened.
    let mut body = b"server exploded ".to_vec();
    body.resize(MAX_ERROR_BODY_BYTES + 1, b'x');
    let server = RecordingServer::start(ResponsePlan::without_content_length(500, body)).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();
    let message = error.to_string();

    assert_eq!(error.kind, ApiErrorKind::Server);
    assert!(message.contains("server exploded"));
    assert!(message.len() < 260);
}

#[tokio::test]
async fn oversized_success_body_is_rejected_without_waiting_for_it() {
    // If a successful response's declared body can bypass the client limit, a broken
    // server can retain the dashboard indefinitely or consume unbounded memory.
    let mut server = RecordingServer::start(ResponsePlan::status_then_wait_with_length(
        200,
        64 * 1024 * 1024,
    ))
    .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(10),
    ))
    .unwrap();

    let error = tokio::time::timeout(Duration::from_secs(2), client.fetch_report(&selection()))
        .await
        .expect("oversized response must be rejected before its body arrives")
        .unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("body is too large"));
    server.take_request().await;
    tokio::time::timeout(Duration::from_secs(2), server.finish())
        .await
        .expect("rejecting an oversized response must release its socket")
        .unwrap();
}

#[tokio::test]
async fn oversized_error_body_is_classified_without_waiting_for_it() {
    // If an unauthenticated error body must be fully buffered before status
    // classification, an oversized diagnostic can turn a 500 into a timeout.
    let mut server = RecordingServer::start(ResponsePlan::status_then_wait_with_length(
        500,
        64 * 1024 * 1024,
    ))
    .await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(10),
    ))
    .unwrap();

    let error = tokio::time::timeout(Duration::from_secs(2), client.fetch_report(&selection()))
        .await
        .expect("oversized error must be classified before its body arrives")
        .unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Server);
    server.take_request().await;
    tokio::time::timeout(Duration::from_secs(2), server.finish())
        .await
        .expect("classifying an oversized error must release its socket")
        .unwrap();
}

#[tokio::test]
async fn redirects_are_reported_without_following_the_location() {
    // If redirect following is re-enabled, bearer credentials can cross origins and a
    // fixed AgentsView endpoint can silently become a different service.
    let server =
        RecordingServer::start(ResponsePlan::redirect("http://192.0.2.1/credential-sink")).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Protocol);
    assert!(error.to_string().contains("redirect"));
}

#[tokio::test]
async fn incomplete_body_is_a_network_error_not_partial_data() {
    // If a truncated successful response is parsed as partial data, summary totals and
    // session rows can disagree without a visible transport failure.
    let server = RecordingServer::start(ResponsePlan::incomplete(b"{\"schema_version\":5")).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(2),
    ))
    .unwrap();

    let error = client.fetch_report(&selection()).await.unwrap_err();

    assert_eq!(error.kind, ApiErrorKind::Network);
}

#[tokio::test]
async fn aborting_an_inflight_fetch_closes_the_test_connection() {
    // If fetch futures keep the socket alive after cancellation, changing filters can
    // accumulate obsolete requests that outlive their dashboard generation.
    let mut server = RecordingServer::start(ResponsePlan::wait_for_disconnect()).await;
    let client = ActivityClient::new(&config(
        server.base_url().clone(),
        None,
        Duration::from_secs(10),
    ))
    .unwrap();
    let fetch = tokio::spawn(async move { client.fetch_report(&selection()).await });

    server.take_request().await;
    fetch.abort();
    assert!(fetch.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), server.finish())
        .await
        .expect("cancelled fetch must close its socket")
        .unwrap();
}
