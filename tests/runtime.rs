// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use herdr_agentsview::app::{App, InputKey, Loadable, MetadataKind, ReportState};
use herdr_agentsview::config::PluginConfig;
use herdr_agentsview::tui::Runtime;
use herdr_agentsview::wire::ReportSelection;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const REPORT: &str = include_str!("fixtures/report-v5.json");
const PROJECTS: &str = r#"{"projects":[{"name":"project-alpha","session_count":2}]}"#;
const AGENTS: &str = r#"{"agents":[{"name":"codex","session_count":2}]}"#;
const MACHINES: &str = r#"{"machines":["machine-alpha"]}"#;

#[derive(Clone, Copy)]
enum Route {
    Report(usize),
    Projects(usize),
    Agents,
    Machines,
}

#[derive(Default)]
struct ServerState {
    requests: Mutex<Vec<String>>,
    report_count: AtomicUsize,
    project_count: AtomicUsize,
    cancellations: AtomicUsize,
    gated_reports: Mutex<BTreeSet<usize>>,
    hold_all: AtomicBool,
    fail_first_report: AtomicBool,
    fail_first_projects: AtomicBool,
    gate: Notify,
}

struct RecordingServer {
    address: std::net::SocketAddr,
    state: Arc<ServerState>,
    task: JoinHandle<()>,
}

impl RecordingServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind recording server");
        let address = listener.local_addr().expect("read recording address");
        let state = Arc::new(ServerState::default());
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let request_state = Arc::clone(&task_state);
                tokio::spawn(async move {
                    serve_request(stream, request_state).await;
                });
            }
        });
        Self {
            address,
            state,
            task,
        }
    }

    fn config(&self, refresh_interval: Duration) -> PluginConfig {
        PluginConfig {
            api_base_url: format!("http://{}/", self.address).parse().unwrap(),
            request_timeout: Some(Duration::from_secs(2)),
            refresh_interval,
            timezone: "America/New_York".parse().unwrap(),
            auth: None,
        }
    }

    fn paths(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }

    fn gate_report(&self, ordinal: usize) {
        self.state.gated_reports.lock().unwrap().insert(ordinal);
    }

    fn gate_reports(&self, ordinals: impl IntoIterator<Item = usize>) {
        self.state.gated_reports.lock().unwrap().extend(ordinals);
    }

    fn release_gated_request(&self) {
        self.state.gate.notify_one();
    }

    fn hold_all_requests(&self) {
        self.state.hold_all.store(true, Ordering::SeqCst);
    }

    fn fail_first_projects_request(&self) {
        self.state.fail_first_projects.store(true, Ordering::SeqCst);
    }

    fn fail_first_report_request(&self) {
        self.state.fail_first_report.store(true, Ordering::SeqCst);
    }
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_request(mut stream: TcpStream, state: Arc<ServerState>) {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut chunk).await.expect("read request");
        if count == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request = String::from_utf8(bytes).expect("request is UTF-8");
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request target")
        .to_owned();
    state.requests.lock().unwrap().push(path.clone());
    let route = if path.starts_with("/api/v1/activity/report") {
        Route::Report(state.report_count.fetch_add(1, Ordering::SeqCst) + 1)
    } else if path.starts_with("/api/v1/projects") {
        Route::Projects(state.project_count.fetch_add(1, Ordering::SeqCst) + 1)
    } else if path.starts_with("/api/v1/agents") {
        Route::Agents
    } else if path.starts_with("/api/v1/machines") {
        Route::Machines
    } else {
        panic!("unexpected request path {path}");
    };

    let gated = state.hold_all.load(Ordering::SeqCst)
        || matches!(route, Route::Report(ordinal) if state.gated_reports.lock().unwrap().contains(&ordinal));
    if gated {
        let mut eof = [0_u8; 1];
        tokio::select! {
            _ = state.gate.notified() => {}
            result = stream.read(&mut eof) => {
                if matches!(result, Ok(0)) {
                    state.cancellations.fetch_add(1, Ordering::SeqCst);
                }
                return;
            }
        }
    }

    let (status, body) = match route {
        Route::Report(1) if state.fail_first_report.load(Ordering::SeqCst) => (
            "503 Service Unavailable",
            "temporarily unavailable".to_owned(),
        ),
        Route::Report(ordinal) => ("200 OK", report_for_path(&path, ordinal)),
        Route::Projects(1) if state.fail_first_projects.load(Ordering::SeqCst) => (
            "503 Service Unavailable",
            "temporarily unavailable".to_owned(),
        ),
        Route::Projects(_) => ("200 OK", PROJECTS.to_owned()),
        Route::Agents => ("200 OK", AGENTS.to_owned()),
        Route::Machines => ("200 OK", MACHINES.to_owned()),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("write response");
}

fn report_for_path(path: &str, ordinal: usize) -> String {
    let mut report: serde_json::Value = serde_json::from_str(REPORT).unwrap();
    if path.contains("date=2026-08-08") {
        report["totals"]["sessions"] = serde_json::json!(7 + ordinal);
    }
    serde_json::to_string(&report).unwrap()
}

fn app(config: &PluginConfig, date: NaiveDate) -> App {
    App::new(
        ReportSelection::new(date, config.timezone),
        config.refresh_interval,
    )
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition timed out");
}

#[tokio::test]
async fn initial_load_requests_report_and_all_metadata() {
    // If startup orchestration drops an endpoint, part of the dashboard remains loading forever.
    let server = RecordingServer::start().await;
    let config = server.config(Duration::from_secs(60));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 9).unwrap());
    let mut runtime = Runtime::new(&config).unwrap();

    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
            && matches!(app.projects(), Loadable::Ready(_))
            && matches!(app.agents(), Loadable::Ready(_))
            && matches!(app.machines(), Loadable::Ready(_))
    })
    .await;

    let endpoints = server
        .paths()
        .into_iter()
        .map(|path| path.split('?').next().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        endpoints,
        BTreeSet::from([
            "/api/v1/activity/report".to_owned(),
            "/api/v1/agents".to_owned(),
            "/api/v1/machines".to_owned(),
            "/api/v1/projects".to_owned(),
        ])
    );
}

#[tokio::test]
async fn foreground_load_cancels_the_old_generation() {
    // If a superseded request survives, a slow old filter can overwrite the newest report.
    let server = RecordingServer::start().await;
    server.gate_report(1);
    let config = server.config(Duration::from_secs(60));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 7).unwrap());
    let mut runtime = Runtime::new(&config).unwrap();
    runtime.start(&mut app);
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 1).await;

    let commands = app.handle_input(
        InputKey::Right,
        NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(),
    );
    runtime.dispatch_all(commands);
    wait_until(|| {
        runtime.drain_events(&mut app);
        app.report()
            .is_some_and(|report| report.totals.sessions == 9)
    })
    .await;
    wait_until(|| server.state.cancellations.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test]
async fn scheduled_refresh_supersedes_an_unresponsive_report() {
    // If a periodic refresh cannot supersede an unresponsive predecessor, an unbounded client
    // request leaves the dashboard refreshing forever and blocks every later scheduled read.
    let server = RecordingServer::start().await;
    server.gate_report(2);
    let mut config = server.config(Duration::from_secs(1));
    config.request_timeout = None;
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
    let started_at = Instant::now();
    let mut runtime = Runtime::new_at(&config, started_at).unwrap();
    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
    })
    .await;

    runtime
        .tick(&mut app, started_at + Duration::from_secs(1))
        .unwrap();
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 2).await;
    runtime
        .tick(&mut app, started_at + Duration::from_secs(2))
        .unwrap();
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
            && app
                .report()
                .is_some_and(|report| report.totals.sessions == 10)
    })
    .await;
    wait_until(|| server.state.cancellations.load(Ordering::SeqCst) == 1).await;
    assert_eq!(server.state.report_count.load(Ordering::SeqCst), 3);

    runtime.dispatch_all(app.handle_input(
        InputKey::Char('r'),
        NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(),
    ));
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 4).await;
}

#[tokio::test]
async fn scheduled_refresh_backoff_allows_a_slow_replacement_to_finish() {
    // If every refresh interval cancels its predecessor, a finite report slower than the
    // interval is starved forever instead of receiving a progressively larger window.
    let server = RecordingServer::start().await;
    server.gate_reports([2, 3]);
    let mut config = server.config(Duration::from_secs(1));
    config.request_timeout = None;
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
    let started_at = Instant::now();
    let mut runtime = Runtime::new_at(&config, started_at).unwrap();
    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
    })
    .await;

    runtime
        .tick(&mut app, started_at + Duration::from_secs(1))
        .unwrap();
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 2).await;
    runtime
        .tick(&mut app, started_at + Duration::from_secs(2))
        .unwrap();
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 3).await;
    wait_until(|| server.state.cancellations.load(Ordering::SeqCst) == 1).await;
    runtime
        .tick(&mut app, started_at + Duration::from_secs(3))
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(server.state.report_count.load(Ordering::SeqCst), 3);
    assert_eq!(server.state.cancellations.load(Ordering::SeqCst), 1);

    server.release_gated_request();
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
            && app
                .report()
                .is_some_and(|report| report.totals.sessions == 10)
    })
    .await;
}

#[tokio::test]
async fn scheduled_refresh_supersedes_an_unresponsive_initial_load() {
    // If startup is excluded from scheduled recovery, an unbounded first request can leave
    // every dashboard region spinning forever even though a replacement would succeed.
    let server = RecordingServer::start().await;
    server.gate_report(1);
    let mut config = server.config(Duration::from_secs(1));
    config.request_timeout = None;
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
    let started_at = Instant::now();
    let mut runtime = Runtime::new_at(&config, started_at).unwrap();
    runtime.start(&mut app);
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 1).await;

    runtime
        .tick(&mut app, started_at + Duration::from_secs(1))
        .unwrap();
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
            && app
                .report()
                .is_some_and(|report| report.totals.sessions == 9)
    })
    .await;
    wait_until(|| server.state.cancellations.load(Ordering::SeqCst) == 1).await;
}

#[tokio::test]
async fn configured_timeout_owns_recovery_without_scheduled_cancellation() {
    // If periodic recovery also cancels an explicitly bounded request, a refresh interval
    // shorter than the configured timeout prevents the operator's timeout from taking effect.
    let server = RecordingServer::start().await;
    server.gate_report(2);
    let config = server.config(Duration::from_secs(1));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
    let started_at = Instant::now();
    let mut runtime = Runtime::new_at(&config, started_at).unwrap();
    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
    })
    .await;

    runtime
        .tick(&mut app, started_at + Duration::from_secs(1))
        .unwrap();
    wait_until(|| server.state.report_count.load(Ordering::SeqCst) == 2).await;
    runtime
        .tick(&mut app, started_at + Duration::from_secs(2))
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(server.state.report_count.load(Ordering::SeqCst), 2);
    assert_eq!(server.state.cancellations.load(Ordering::SeqCst), 0);

    server.release_gated_request();
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
            && app
                .report()
                .is_some_and(|report| report.totals.sessions == 9)
    })
    .await;
}

#[tokio::test]
async fn scheduled_refresh_recovers_from_a_transient_initial_failure() {
    // If periodic refresh ignores a failed initial report, a short API outage leaves the
    // dashboard unavailable forever even after the endpoint recovers.
    let server = RecordingServer::start().await;
    server.fail_first_report_request();
    let config = server.config(Duration::from_secs(1));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
    let started_at = Instant::now();
    let mut runtime = Runtime::new_at(&config, started_at).unwrap();
    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Failed(_))
    })
    .await;

    runtime
        .tick(&mut app, started_at + Duration::from_secs(1))
        .unwrap();

    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.report_state(), ReportState::Ready { .. })
    })
    .await;
    assert_eq!(server.state.report_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dropping_runtime_aborts_every_owned_request() {
    // If runtime shutdown detaches requests, closing the pane leaves network work running.
    let server = RecordingServer::start().await;
    server.hold_all_requests();
    let config = server.config(Duration::from_secs(60));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 9).unwrap());
    let mut runtime = Runtime::new(&config).unwrap();

    runtime.start(&mut app);
    wait_until(|| server.paths().len() == 4).await;
    drop(runtime);

    wait_until(|| server.state.cancellations.load(Ordering::SeqCst) == 4).await;
}

#[tokio::test]
async fn failed_metadata_can_be_retried_independently() {
    // If metadata retry is coupled to the report, one failed selector reloads or discards good data.
    let server = RecordingServer::start().await;
    server.fail_first_projects_request();
    let config = server.config(Duration::from_secs(60));
    let mut app = app(&config, NaiveDate::from_ymd_opt(2026, 8, 9).unwrap());
    let mut runtime = Runtime::new(&config).unwrap();
    runtime.start(&mut app);
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.projects(), Loadable::Failed(_))
    })
    .await;

    runtime.dispatch(app.retry_metadata(MetadataKind::Projects));
    wait_until(|| {
        runtime.drain_events(&mut app);
        matches!(app.projects(), Loadable::Ready(projects) if projects.len() == 1)
    })
    .await;
    assert!(matches!(app.report_state(), ReportState::Ready { .. }));
}
