// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Utc};
use herdr_agentsview::api::{ApiError, ApiErrorKind};
use herdr_agentsview::app::App;
use herdr_agentsview::render::ColorMode;
use herdr_agentsview::wire::{AgentInfo, Bucket, Money, ProjectInfo, Report};

use super::activity::{received_at, report, selection};

pub fn render_time() -> DateTime<Utc> {
    "2026-08-08T17:21:12Z".parse().unwrap()
}

pub fn ready_app(color_mode: ColorMode) -> App {
    app_with_report(expanded_report(), color_mode)
}

pub fn loading_app(color_mode: ColorMode) -> App {
    let mut app = new_app(color_mode);
    app.begin_foreground_load();
    app
}

pub fn stale_partial_app(color_mode: ColorMode) -> App {
    let mut app = app_with_report(partial_report(), color_mode);
    app.begin_refresh().unwrap();
    app.apply_report(
        Err(ApiError::timeout()),
        "2026-08-08T17:31:12Z".parse().unwrap(),
    );
    app
}

pub fn authentication_error_app(color_mode: ColorMode) -> App {
    failed_app(
        ApiErrorKind::Authentication,
        "AgentsView requires authentication (HTTP 401); configure a runtime token",
        color_mode,
    )
}

pub fn failed_app(kind: ApiErrorKind, message: &str, color_mode: ColorMode) -> App {
    let mut app = new_app(color_mode);
    app.begin_foreground_load();
    app.apply_report(
        Err(ApiError {
            kind,
            message: message.to_owned(),
        }),
        received_at(),
    );
    app
}

pub fn metadata_degraded_app(color_mode: ColorMode) -> App {
    let mut app = ready_app(color_mode);
    app.apply_projects(Err(ApiError {
        kind: ApiErrorKind::Network,
        message: "projects unavailable".to_owned(),
    }));
    app
}

pub fn empty_app(color_mode: ColorMode) -> App {
    let mut value = expanded_report();
    value.peak.agents = 0;
    value.peak.at = None;
    value.totals.active_minutes = 0.0;
    value.totals.idle_minutes =
        (value.effective_end - value.range_start).num_seconds() as f64 / 60.0;
    value.totals.agent_minutes = 0.0;
    value.totals.sessions = 0;
    value.totals.untimed_sessions = 0;
    value.totals.distinct_projects = 0;
    value.totals.distinct_models = 0;
    value.totals.output_tokens = 0;
    value.totals.cost = Money { microdollars: 0 };
    value.totals.automated_agent_minutes = 0.0;
    value.totals.interactive_agent_minutes = 0.0;
    value.totals.automated_cost = Money { microdollars: 0 };
    value.totals.interactive_cost = Money { microdollars: 0 };
    value.totals.automated_sessions = 0;
    value.totals.interactive_sessions = 0;
    value.by_project.clear();
    value.by_model.clear();
    value.by_agent.clear();
    value.by_session.clear();
    value.intervals.clear();
    for bucket in &mut value.buckets {
        bucket.max_agents = 0;
        bucket.agent_minutes = 0.0;
        bucket.output_tokens = 0;
        bucket.cost = Money { microdollars: 0 };
        bucket.automated_at_peak = 0;
        bucket.interactive_at_peak = 0;
    }
    app_with_report(value, color_mode)
}

pub fn assert_golden(path: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(path);
    assert_eq!(actual, fs::read_to_string(path).unwrap());
}

pub fn app_with_report(value: Report, color_mode: ColorMode) -> App {
    let mut app = new_app(color_mode);
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value)), received_at());
    app.apply_projects(Ok(vec![
        ProjectInfo {
            name: "project-alpha".to_owned(),
            session_count: 1,
        },
        ProjectInfo {
            name: "project-beta".to_owned(),
            session_count: 1,
        },
    ]));
    app.apply_agents(Ok(vec![
        AgentInfo {
            name: "codex".to_owned(),
            session_count: 2,
        },
        AgentInfo {
            name: "reviewer".to_owned(),
            session_count: 1,
        },
    ]));
    app.apply_machines(Ok(vec!["machine-alpha".to_owned()]));
    app
}

fn new_app(color_mode: ColorMode) -> App {
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_color_mode(color_mode);
    app
}

fn expanded_report() -> Report {
    let mut value = report();
    let start: DateTime<FixedOffset> = "2026-08-08T00:00:00-04:00".parse().unwrap();
    let levels = [
        (0, 0),
        (0, 0),
        (0, 0),
        (0, 0),
        (1, 0),
        (2, 1),
        (3, 1),
        (4, 2),
        (5, 2),
        (4, 3),
        (5, 1),
        (3, 2),
        (4, 1),
        (2, 1),
        (0, 0),
        (0, 0),
        (2, 0),
        (3, 1),
        (2, 2),
        (1, 1),
        (0, 0),
        (0, 0),
        (1, 0),
        (0, 0),
        (0, 0),
        (2, 1),
        (3, 2),
        (4, 2),
        (3, 1),
        (2, 1),
        (1, 0),
        (0, 0),
    ];
    let template = value.buckets[0].clone();
    value.buckets = levels
        .iter()
        .enumerate()
        .map(|(index, (interactive, automated))| {
            bucket(
                &template,
                start + chrono::Duration::minutes(index as i64 * 45),
                *interactive,
                *automated,
            )
        })
        .collect();
    value.range_start = start;
    value.range_end = value.buckets.last().unwrap().end;
    value.effective_end = value.range_end;
    value.bucket_seconds = 45 * 60;
    value.bucket_count = value.buckets.len();
    value.elapsed_bucket_count = value.buckets.len();
    value.partial = false;
    value.as_of = None;
    value.peak.agents = 7;
    value.peak.at = Some(value.buckets[8].start);
    value
}

fn partial_report() -> Report {
    let mut value = expanded_report();
    let observed = 22;
    value.partial = true;
    value.elapsed_bucket_count = observed;
    value.effective_end = value.buckets[observed - 1].end;
    value.as_of = Some(value.effective_end);
    value.buckets[20].max_agents = 0;
    value.buckets[20].interactive_at_peak = 0;
    value.buckets[20].automated_at_peak = 0;
    value
}

fn bucket(
    template: &Bucket,
    start: DateTime<FixedOffset>,
    interactive: usize,
    automated: usize,
) -> Bucket {
    let mut value = template.clone();
    value.start = start;
    value.end = start + chrono::Duration::minutes(45);
    value.max_agents = interactive + automated;
    value.agent_minutes = (interactive + automated) as f64 * 30.0;
    value.output_tokens = (interactive + automated) as u64 * 100;
    value.cost = Money {
        microdollars: (interactive + automated) as i64 * 250_000,
    };
    value.interactive_at_peak = interactive;
    value.automated_at_peak = automated;
    value
}
