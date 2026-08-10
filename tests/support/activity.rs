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
    serde_json::from_str(include_str!("../fixtures/report-v5.json")).unwrap()
}

pub fn received_at() -> DateTime<Utc> {
    "2026-08-08T17:21:00Z".parse().unwrap()
}

pub fn ready_app() -> App {
    let mut app = App::new(selection(), Duration::from_secs(300));
    let request = app.begin_foreground_load();
    app.apply_report(request.generation, Ok(Box::new(report())), received_at());
    app
}
