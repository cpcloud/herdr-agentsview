// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use herdr_agentsview::app::App;
use herdr_agentsview::wire::{Report, ReportSelection};

pub fn selection() -> ReportSelection {
    ReportSelection::new(
        NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
        "America/New_York".parse().unwrap(),
    )
}

pub fn report() -> Report {
    serde_json::from_str(include_str!("../fixtures/report-v6.json")).unwrap()
}

pub fn received_at() -> DateTime<Utc> {
    "2026-08-08T17:21:00Z".parse().unwrap()
}

pub fn ready_app() -> App {
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(report())), received_at());
    app
}
