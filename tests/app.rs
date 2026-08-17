// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use herdr_agentsview::api::{ApiError, ApiErrorKind, SessionFetch};
use herdr_agentsview::app::{
    App, AppCommand, BreakdownCategory, BreakdownValue, Focus, InputKey, Loadable, MetadataKind,
    ReportState, SessionSortColumn, SortDirection,
};
use herdr_agentsview::wire::{ProjectInfo, SessionRow};

#[path = "support/activity.rs"]
mod activity_support;

use activity_support::{ready_app, received_at, report, selection};

fn error(kind: ApiErrorKind, message: &str) -> ApiError {
    ApiError {
        kind,
        message: message.to_owned(),
    }
}

fn session_ids(app: &App) -> Vec<&str> {
    app.sorted_sessions()
        .into_iter()
        .map(|row| row.session_id.as_str())
        .collect()
}

fn selected_session_id(app: &App) -> &str {
    let rows = app.sorted_sessions();
    rows[app.session_cursor()].session_id.as_str()
}

#[test]
fn initial_load_transitions_to_ready_without_invented_data() {
    // If the initial state contains a zero-shaped report, loading can look like a real
    // inactive day and later render paths cannot distinguish the two.
    let mut app = App::new(selection(), Duration::from_secs(300));
    assert!(matches!(app.report_state(), ReportState::InitialLoading));
    assert!(app.report().is_none());

    let request_selection = app.begin_foreground_load();
    assert_eq!(request_selection, selection());
    app.apply_report(Ok(Box::new(report())), received_at());

    assert!(matches!(
        app.report_state(),
        ReportState::Ready { received_at: value, .. } if *value == received_at()
    ));
    assert_eq!(app.report().unwrap().totals.sessions, 3);
}

#[test]
fn initial_timeline_cursor_starts_at_the_first_nonzero_bucket() {
    // If the inactive cursor remains at midnight, the first slice activation can select a
    // quiet interval instead of the day's first useful session bucket.
    let mut value = report();
    value.buckets[0].interactive_at_peak = 0;
    value.buckets[0].automated_at_peak = 0;
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();

    app.apply_report(Ok(Box::new(value)), received_at());

    assert_eq!(app.timeline_cursor(), 1);
    app.toggle_timeline_inspection();
    assert_eq!(app.timeline_cursor(), 1);
}

#[test]
fn timeline_cursor_can_move_into_leading_zero_buckets_after_initial_positioning() {
    // If first-use positioning also becomes a navigation boundary, operators cannot inspect
    // quiet buckets earlier in the day even though the full-day chart still shows them.
    let mut value = report();
    value.buckets[0].interactive_at_peak = 0;
    value.buckets[0].automated_at_peak = 0;
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value)), received_at());
    app.toggle_timeline_inspection();

    app.move_timeline(-1);

    assert_eq!(app.timeline_cursor(), 0);
}

#[test]
fn refresh_preserves_an_active_slice_on_a_leading_zero_bucket() {
    // If refresh reapplies first-use positioning, an operator inspecting an earlier quiet
    // bucket is silently moved or kicked out of the session slice.
    let mut value = report();
    value.buckets[0].interactive_at_peak = 0;
    value.buckets[0].automated_at_peak = 0;
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value.clone())), received_at());
    app.toggle_timeline_inspection();
    app.move_timeline(-1);
    app.begin_refresh().unwrap();

    app.apply_report(Ok(Box::new(value)), received_at());

    assert!(app.timeline_inspection_active());
    assert_eq!(app.timeline_cursor(), 0);
}

#[test]
fn refresh_retains_data_and_rejects_overlap() {
    // If refresh hides the last good report or overlaps itself, a healthy dashboard flickers
    // and can accumulate redundant requests on slow links.
    let mut app = ready_app();
    let request_selection = app.begin_refresh().unwrap();

    assert_eq!(request_selection, *app.selection());
    assert!(app.report().is_some());
    assert!(matches!(app.report_state(), ReportState::Refreshing { .. }));
    assert!(app.begin_refresh().is_none());
}

#[test]
fn rejected_refresh_preserves_the_active_request() {
    // If rejecting an overlapping refresh loses the active request, its valid response is
    // discarded and the dashboard remains stuck in a report-less loading state.
    let mut app = ready_app();
    app.begin_refresh().unwrap();

    assert!(app.begin_refresh().is_none());
    assert!(matches!(app.report_state(), ReportState::Refreshing { .. }));

    app.apply_report(Ok(Box::new(report())), received_at());

    assert!(matches!(app.report_state(), ReportState::Ready { .. }));
}

#[test]
fn failed_refresh_keeps_last_good_report_and_marks_it_stale() {
    // If a refresh error drops the report, a transient network failure destroys the exact
    // data needed for a useful stale-data state.
    let mut app = ready_app();
    app.begin_refresh().unwrap();

    app.apply_report(Err(ApiError::timeout()), received_at());

    assert!(matches!(
        app.report_state(),
        ReportState::Stale { error, .. } if error.kind == ApiErrorKind::Timeout
    ));
    assert!(app.report().is_some());
}

#[test]
fn foreground_failures_preserve_the_boundary_classification() {
    // If auth, network, and protocol failures collapse into an empty report, the recovery
    // action shown to the operator will be wrong.
    for kind in [
        ApiErrorKind::Authentication,
        ApiErrorKind::Network,
        ApiErrorKind::Protocol,
    ] {
        let mut app = App::new(selection(), Duration::from_secs(300));
        app.begin_foreground_load();
        app.apply_report(Err(error(kind, "boundary failed")), received_at());

        assert!(matches!(
            app.report_state(),
            ReportState::Failed(value) if value.kind == kind
        ));
        assert!(app.report().is_none());
    }
}

#[test]
fn scheduled_load_retries_only_transient_foreground_failures() {
    // If scheduled recovery retries permanent failures, a bad token or incompatible API is
    // hammered forever; if it ignores transient failures, a brief outage becomes permanent.
    for kind in [
        ApiErrorKind::Timeout,
        ApiErrorKind::Network,
        ApiErrorKind::Server,
    ] {
        let mut app = App::new(selection(), Duration::from_secs(300));
        app.begin_foreground_load();
        app.apply_report(Err(error(kind, "transient failure")), received_at());

        assert_eq!(app.begin_scheduled_load().unwrap(), *app.selection());
    }

    for kind in [
        ApiErrorKind::Authentication,
        ApiErrorKind::Forbidden,
        ApiErrorKind::Protocol,
    ] {
        let mut app = App::new(selection(), Duration::from_secs(300));
        app.begin_foreground_load();
        app.apply_report(Err(error(kind, "permanent failure")), received_at());

        assert!(app.begin_scheduled_load().is_none());
        assert!(matches!(app.report_state(), ReportState::Failed(_)));
    }
}

#[test]
fn partial_and_empty_reports_remain_distinct_ready_states() {
    // If partial fields or an empty result are normalized away, future buckets can look like
    // observed zeroes and a real empty day can look like a transport failure.
    let mut partial = ready_app();
    assert!(partial.report().unwrap().partial);
    assert_eq!(partial.report().unwrap().elapsed_bucket_count, 2);
    assert!(partial.report().unwrap().as_of.is_some());
    assert!(!partial.is_empty());

    partial.begin_foreground_load();
    let mut empty = report();
    empty.totals.sessions = 0;
    empty.by_session.clear();
    empty.sessions_total = 0;
    empty.by_project.clear();
    empty.by_model.clear();
    empty.by_agent.clear();
    partial.apply_report(Ok(Box::new(empty)), received_at());

    assert!(partial.is_empty());
    assert!(matches!(partial.report_state(), ReportState::Ready { .. }));
}

#[test]
fn metadata_failures_are_independent_from_each_other_and_the_report() {
    // If metadata shares one load state, an agent-list failure can erase a successful report
    // or disable unrelated Project and Machine selectors.
    let mut app = ready_app();
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "project-alpha".to_owned(),
        session_count: 2,
    }]));
    app.apply_agents(Err(error(ApiErrorKind::Network, "agents unavailable")));
    app.apply_machines(Ok(vec!["machine-alpha".to_owned()]));

    assert!(matches!(app.projects(), Loadable::Ready(values) if values.len() == 1));
    assert!(matches!(app.agents(), Loadable::Failed(value) if value.kind == ApiErrorKind::Network));
    assert!(matches!(app.machines(), Loadable::Ready(values) if values.len() == 1));
    assert!(app.report().is_some());
}

#[test]
fn metadata_retry_resets_only_the_failed_selector() {
    // If retrying one selector resets all metadata, a localized failure causes avoidable
    // loading states across the entire filter row.
    let mut app = ready_app();
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "project-alpha".to_owned(),
        session_count: 2,
    }]));
    app.apply_agents(Err(error(ApiErrorKind::Network, "agents unavailable")));

    let command = app.retry_metadata(MetadataKind::Agents);

    assert_eq!(command, AppCommand::FetchMetadata(MetadataKind::Agents));
    assert!(matches!(app.agents(), Loadable::Loading));
    assert!(matches!(app.projects(), Loadable::Ready(_)));
}

#[test]
fn session_sort_is_stable_and_keeps_untimed_rows_last_in_both_directions() {
    // If null ordering follows direction reversal, untimed rows jump above measured sessions;
    // if ties are unstable, selection moves between refreshes.
    let mut app = ready_app();
    app.begin_foreground_load();
    let mut value = report();
    value.by_session[0].agent_minutes = Some(10.0);
    value.by_session[1].agent_minutes = Some(10.0);
    app.apply_report(Ok(Box::new(value)), received_at());

    assert_eq!(app.sort_column(), SessionSortColumn::AgentMinutes);
    assert_eq!(app.sort_direction(), SortDirection::Descending);
    assert_eq!(
        session_ids(&app),
        ["session-alpha", "session-beta", "session-gamma"]
    );

    app.toggle_sort_direction();
    assert_eq!(
        session_ids(&app),
        ["session-alpha", "session-beta", "session-gamma"]
    );
}

#[test]
fn every_session_sort_uses_its_visible_field_and_keeps_partial_windows_last() {
    // If a comparator arm reads the wrong field, flips its stable tie-break, or treats a
    // half-timed row as measured, table ordering disagrees with the selected header.
    let mut value = report();
    value.by_session[2].first_active = value.by_session[0].first_active;
    assert!(value.by_session[2].last_active.is_none());

    let cases = [
        (
            SessionSortColumn::Session,
            ["session-alpha", "session-gamma", "session-beta"],
            ["session-beta", "session-gamma", "session-alpha"],
        ),
        (
            SessionSortColumn::Model,
            ["session-alpha", "session-beta", "session-gamma"],
            ["session-gamma", "session-beta", "session-alpha"],
        ),
        (
            SessionSortColumn::Project,
            ["session-alpha", "session-beta", "session-gamma"],
            ["session-gamma", "session-beta", "session-alpha"],
        ),
        (
            SessionSortColumn::Agent,
            ["session-alpha", "session-gamma", "session-beta"],
            ["session-beta", "session-alpha", "session-gamma"],
        ),
        (
            SessionSortColumn::Cost,
            ["session-gamma", "session-beta", "session-alpha"],
            ["session-alpha", "session-beta", "session-gamma"],
        ),
        (
            SessionSortColumn::Window,
            ["session-alpha", "session-beta", "session-gamma"],
            ["session-beta", "session-alpha", "session-gamma"],
        ),
    ];

    for (column, ascending, descending) in cases {
        let mut app = App::new(selection(), Duration::from_secs(300));
        app.begin_foreground_load();
        app.apply_report(Ok(Box::new(value.clone())), received_at());
        app.set_focus(Focus::Sessions);
        for _ in 0..7 {
            if app.sort_column() == column {
                break;
            }
            app.handle_input(InputKey::Right, selection().date);
        }
        assert_eq!(app.sort_column(), column);

        app.toggle_sort_direction();
        assert_eq!(app.sort_direction(), SortDirection::Ascending);
        assert_eq!(session_ids(&app), ascending, "ascending {column:?}");

        app.toggle_sort_direction();
        assert_eq!(app.sort_direction(), SortDirection::Descending);
        assert_eq!(session_ids(&app), descending, "descending {column:?}");
    }
}

#[test]
fn session_selection_follows_identity_across_sort_and_refresh() {
    // If selection is only a row offset, changing sort order or refreshing changed values
    // silently points the compact details panel at another session.
    let mut app = ready_app();
    app.set_focus(Focus::Sessions);
    app.move_session(1, 3);
    assert_eq!(selected_session_id(&app), "session-beta");

    app.handle_input(InputKey::Right, selection().date);
    app.handle_input(InputKey::Right, selection().date);
    assert_eq!(app.sort_column(), SessionSortColumn::Window);
    assert_eq!(selected_session_id(&app), "session-beta");

    app.handle_input(InputKey::Enter, selection().date);
    assert_eq!(app.sort_direction(), SortDirection::Ascending);
    assert_eq!(selected_session_id(&app), "session-beta");

    app.begin_refresh().unwrap();
    let mut value = report();
    value.by_session[1].first_active = value.by_session[0]
        .first_active
        .map(|timestamp| timestamp - chrono::Duration::hours(1));
    value.by_session[1].last_active = value.by_session[0]
        .last_active
        .map(|timestamp| timestamp - chrono::Duration::hours(1));
    app.apply_report(Ok(Box::new(value)), received_at());

    assert_eq!(selected_session_id(&app), "session-beta");
    assert_eq!(app.session_cursor(), 0);
}

#[test]
fn missing_session_selection_clamps_cursor_and_scroll_after_refresh() {
    // If the selected session disappears, preserving its old offset without clamping can
    // leave the cursor and compact detail row outside the smaller refreshed report.
    let mut app = ready_app();
    app.move_session(2, 1);
    assert_eq!(selected_session_id(&app), "session-gamma");
    assert_eq!(app.session_scroll(), 2);

    app.begin_refresh().unwrap();
    let mut value = report();
    value
        .by_session
        .retain(|row| row.session_id != "session-gamma");
    app.apply_report(Ok(Box::new(value)), received_at());

    assert_eq!(app.session_cursor(), 1);
    assert_eq!(app.session_scroll(), 1);
    assert_eq!(selected_session_id(&app), "session-beta");
}

#[test]
fn session_cursor_and_scroll_clamp_to_visible_rows() {
    // If cursor and scroll are not clamped together, resize or refresh can leave selection
    // outside the rendered table.
    let mut app = ready_app();

    app.move_session(20, 2);
    assert_eq!(app.session_cursor(), 2);
    assert_eq!(app.session_scroll(), 1);
    app.move_session(-20, 2);
    assert_eq!(app.session_cursor(), 0);
    assert_eq!(app.session_scroll(), 0);
}

#[test]
fn sliced_session_navigation_clamps_and_scrolls_to_the_sliced_rows() {
    // If session navigation counts the full report while a bucket slice is active, moving
    // down can put the selected row beyond the sliced viewport and make its marker vanish.
    let mut value = report();
    let template = value.by_session[0].clone();
    value.by_session.clear();
    let mut bucket_rows = Vec::new();
    for index in 0..6 {
        let mut row = template.clone();
        row.session_id = format!("session-{index}");
        row.title = format!("Session {index}");
        row.agent_minutes = Some((6 - index) as f64);
        value.by_session.push(row);
        if index < 4 {
            bucket_rows.push(value.by_session[index].clone());
        }
    }
    value.totals.sessions = value.by_session.len();
    value.sessions_total = value.by_session.len();
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value)), received_at());
    app.toggle_timeline_inspection();
    apply_bucket_rows(&mut app, bucket_rows);

    app.move_session(20, 2);

    assert_eq!(app.session_cursor(), 3);
    assert_eq!(app.session_scroll(), 2);
}

#[test]
fn moving_a_slice_preserves_session_identity_or_selects_the_first_survivor() {
    // If a slice move preserves only the raw row offset, the selection can silently jump to
    // an unrelated session or remain beyond the destination bucket's displayed rows.
    let shared = report();
    let alpha = session(&shared, "session-alpha");
    let beta = session(&shared, "session-beta");
    let mut preserving = App::new(selection(), Duration::from_secs(300));
    preserving.begin_foreground_load();
    preserving.apply_report(Ok(Box::new(shared)), received_at());
    preserving.toggle_timeline_inspection();
    apply_bucket_rows(&mut preserving, vec![alpha.clone(), beta.clone()]);
    preserving.move_session(1, 2);
    assert_eq!(selected_session_id(&preserving), "session-beta");

    preserving.move_timeline(1);
    apply_bucket_rows(&mut preserving, vec![alpha.clone(), beta]);

    assert_eq!(preserving.session_cursor(), 1);
    assert_eq!(selected_session_id(&preserving), "session-beta");

    let mut falling_back = ready_app();
    falling_back.toggle_timeline_inspection();
    let initial_rows = vec![
        session(falling_back.report().unwrap(), "session-alpha"),
        session(falling_back.report().unwrap(), "session-beta"),
    ];
    apply_bucket_rows(&mut falling_back, initial_rows);
    falling_back.move_session(1, 2);
    assert_eq!(selected_session_id(&falling_back), "session-beta");

    falling_back.move_timeline(1);
    apply_bucket_rows(&mut falling_back, vec![alpha]);

    assert_eq!(falling_back.session_cursor(), 0);
    assert_eq!(selected_session_id(&falling_back), "session-alpha");
}

#[test]
fn revisiting_a_cached_bucket_restores_the_selected_session() {
    // If a cached destination skips both the request and local selection restoration, moving
    // back to it silently jumps the cursor to the first row.
    let value = report();
    let rows = vec![
        session(&value, "session-alpha"),
        session(&value, "session-beta"),
    ];
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value)), received_at());
    app.toggle_timeline_inspection();
    apply_bucket_rows(&mut app, rows.clone());
    app.move_session(1, 2);
    app.move_timeline(1);
    apply_bucket_rows(&mut app, rows);
    assert_eq!(selected_session_id(&app), "session-beta");

    app.move_timeline(-1);

    assert!(app.session_page_request().is_none());
    assert_eq!(selected_session_id(&app), "session-beta");
}

#[test]
fn refresh_that_removes_the_inspected_bucket_exits_session_slicing() {
    // If refresh clamps a removed bucket index but leaves slicing active, the footer and
    // Sessions table silently retarget to a different interval.
    let mut app = ready_app();
    app.toggle_timeline_inspection();
    app.move_timeline(1);
    app.begin_refresh().unwrap();
    let mut value = report();
    value.buckets.truncate(1);
    value.bucket_count = 1;
    value.elapsed_bucket_count = 1;

    app.apply_report(Ok(Box::new(value)), received_at());

    assert!(!app.timeline_inspection_active());
}

#[test]
fn refresh_without_a_page_id_exits_session_slicing() {
    // If a refreshed fallback report loses report_id but retains the old slice mode, its full
    // session rows stay hidden behind a bucket page that the server cannot provide.
    let mut app = ready_app();
    app.toggle_timeline_inspection();
    app.begin_refresh().unwrap();
    let mut replacement = report();
    replacement.report_id = None;

    app.apply_report(Ok(Box::new(replacement)), received_at());

    assert!(!app.timeline_inspection_active());
    assert!(app.session_page_request().is_none());
    assert_eq!(app.sorted_sessions().len(), 3);
}

#[test]
fn breakdown_category_and_value_mode_select_server_computed_rows() {
    // If category or value mode mutates the data source, bars can show Project labels with
    // Model values or use cost while claiming agent-minutes.
    let mut app = ready_app();
    assert_eq!(app.breakdown_category(), BreakdownCategory::Project);
    assert_eq!(app.breakdown_value(), BreakdownValue::AgentMinutes);
    assert_eq!(app.breakdown_rows()[0].key, "project-alpha");

    app.set_breakdown_category(BreakdownCategory::Agent);
    app.toggle_breakdown_value();

    assert_eq!(app.breakdown_value(), BreakdownValue::Cost);
    assert_eq!(app.breakdown_rows()[0].key, "codex");
}

fn session(report: &herdr_agentsview::wire::Report, id: &str) -> SessionRow {
    report
        .by_session
        .iter()
        .find(|row| row.session_id == id)
        .unwrap()
        .clone()
}

fn apply_bucket_rows(app: &mut App, rows: Vec<SessionRow>) {
    let request = app.session_page_request().expect("active bucket request");
    app.apply_session_page(&request, Ok(SessionFetch::Rows(rows)), received_at());
}
