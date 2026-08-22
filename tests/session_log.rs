// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use chrono::NaiveDate;
use herdr_agentsview::api::{ApiError, ApiErrorKind};
use herdr_agentsview::app::{AppCommand, Focus, InputKey, View};
use herdr_agentsview::wire::{Automation, SessionLogPage};

#[path = "support/activity.rs"]
mod activity_support;

use activity_support::{ready_app, selection};

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 9).unwrap()
}

fn session_page() -> SessionLogPage {
    serde_json::from_str(include_str!("fixtures/sessions.json")).unwrap()
}

fn session_log_command(
    command: Option<AppCommand>,
) -> (herdr_agentsview::wire::SessionLogQuery, bool) {
    match command {
        Some(AppCommand::FetchSessionLog { query, append }) => (query, append),
        other => panic!("expected a session log request, got {other:?}"),
    }
}

#[test]
fn o_opens_the_session_log_and_requests_the_list() {
    // If opening the log only switches chrome, the first visit has no rows and looks empty.
    let mut app = ready_app();

    let (query, append) = session_log_command(app.handle_input(InputKey::Char('o'), today()));

    assert_eq!(app.view(), View::SessionLog);
    assert_eq!(app.focus(), Focus::SessionLog);
    assert!(!append);
    assert_eq!(query.date, selection().date);
    assert!(query.include_automated);
    assert!(query.cursor.is_none());
}

#[test]
fn opening_an_already_loaded_log_does_not_refetch() {
    // If re-entering the log always hits the network, leaving and returning flashes loading
    // over data the operator already has.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);
    app.handle_input(InputKey::Escape, today());

    assert_eq!(app.view(), View::Activity);
    assert!(app.handle_input(InputKey::Char('o'), today()).is_none());
    assert_eq!(app.view(), View::SessionLog);
    assert_eq!(app.displayed_session_log().len(), 2);
}

#[test]
fn escape_and_o_leave_the_session_log_without_quitting() {
    // If Esc or o quit the plugin, operators cannot return to Activity after browsing.
    let mut app = ready_app();
    app.set_focus(Focus::Breakdowns);
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);

    assert!(app.handle_input(InputKey::Escape, today()).is_none());
    assert_eq!(app.view(), View::Activity);
    assert_eq!(app.focus(), Focus::Breakdowns);

    app.handle_input(InputKey::Char('o'), today());
    assert!(app.handle_input(InputKey::Char('o'), today()).is_none());
    assert_eq!(app.view(), View::Activity);
    assert_eq!(app.focus(), Focus::Breakdowns);
}

#[test]
fn session_log_hides_children_until_expanded() {
    // If child rows always occupy a line, the compact list loses the parent scan path from
    // the AgentsView sidebar.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);

    let rows = app.displayed_session_log();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].entry.id, "session-root-alpha");
    assert!(rows[0].has_children);
    assert!(!rows[0].expanded);
    assert_eq!(rows[1].entry.id, "session-untitled");

    app.handle_input(InputKey::Enter, today());
    let rows = app.displayed_session_log();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].entry.id, "session-child-alpha");
    assert_eq!(rows[1].depth, 1);
    assert_eq!(rows[1].relationship_marker(), Some("subagent"));
}

#[test]
fn session_log_down_at_the_end_requests_the_opaque_cursor() {
    // If paging parses or drops next_cursor, later roots never appear and the operator
    // cannot tell a short first page from a complete log.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);
    app.set_session_log_viewport_rows(2);
    app.handle_input(InputKey::Down, today());

    let (query, append) = session_log_command(app.handle_input(InputKey::Down, today()));
    assert!(append);
    assert_eq!(query.cursor.as_deref(), Some("opaque-cursor-1"));
}

#[test]
fn automated_filter_hides_interactive_session_log_rows() {
    // Activity's automated selector has no matching list-only query. If the log ignores
    // is_automated after include_automated=true, Interactive and Automated views match.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);
    app.set_automation(Automation::Automated);
    app.apply_session_log(Ok(session_page()), false);

    let ids = app
        .displayed_session_log()
        .into_iter()
        .map(|row| row.entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["session-untitled"]);
}

#[test]
fn session_log_focus_order_skips_activity_regions() {
    // If Tab still walks the hidden timeline and tables, focus disappears while browsing.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());

    let expected = [
        Focus::SessionLog,
        Focus::Date,
        Focus::Project,
        Focus::Agent,
        Focus::Machine,
        Focus::Automation,
        Focus::SessionLog,
    ];
    assert_eq!(app.focus(), expected[0]);
    for focus in &expected[1..] {
        app.handle_input(InputKey::Tab, today());
        assert_eq!(app.focus(), *focus);
    }
}

#[test]
fn q_still_quits_from_the_session_log() {
    // If q is swallowed as a leave key, operators cannot close the plugin while browsing.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    assert_eq!(
        app.handle_input(InputKey::Char('q'), today()),
        Some(AppCommand::Quit)
    );
}

#[test]
fn compact_activity_keys_do_not_leave_the_session_log() {
    // If s/b still retarget Activity compact regions, the operator leaves the log without
    // a leave key and cannot tell which surface is focused.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());

    assert!(app.handle_input(InputKey::Char('s'), today()).is_none());
    assert!(app.handle_input(InputKey::Char('b'), today()).is_none());
    assert_eq!(app.view(), View::SessionLog);
    assert_eq!(app.focus(), Focus::SessionLog);
}

#[test]
fn session_log_failure_retries_without_dropping_activity() {
    // If r reloads only the Activity report, a failed log cannot recover and a good
    // dashboard is discarded as collateral.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(
        Err(ApiError {
            kind: ApiErrorKind::Network,
            message: "session log unavailable".to_owned(),
        }),
        false,
    );

    let (query, append) = session_log_command(app.handle_input(InputKey::Char('r'), today()));
    assert!(!append);
    assert!(query.cursor.is_none());
    assert!(app.report().is_some());
}

#[test]
fn date_change_in_the_session_log_requests_a_new_list() {
    // If filter changes keep the previous day's rows, the log silently disagrees with the
    // date control still shown in the header.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);
    app.set_focus(Focus::Date);

    let command = app.handle_input(InputKey::Left, today());
    assert!(matches!(command, Some(AppCommand::FetchReport(_))));
    assert!(app.session_log_is_loading());
}

#[test]
fn append_keeps_existing_rows_when_the_next_page_arrives() {
    // If a later page replaces the first, scrolling to load more wipes the rows the
    // operator was just reading.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('o'), today());
    app.apply_session_log(Ok(session_page()), false);
    let mut extra = session_page();
    extra.sessions = vec![herdr_agentsview::wire::SessionLogEntry {
        id: "session-root-gamma".to_owned(),
        project: "project-gamma".to_owned(),
        agent: "codex".to_owned(),
        display_name: Some("Later root".to_owned()),
        created_at: "2026-08-08T11:00:00Z".to_owned(),
        message_count: 8,
        ..herdr_agentsview::wire::SessionLogEntry::default()
    }];
    extra.next_cursor = None;
    extra.total = 3;
    app.apply_session_log(Ok(extra), true);

    let ids = app
        .displayed_session_log()
        .into_iter()
        .map(|row| row.entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "session-root-alpha",
            "session-untitled",
            "session-root-gamma"
        ]
    );
}
