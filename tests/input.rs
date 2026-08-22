// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use chrono::{NaiveDate, Utc};
use herdr_agentsview::api::{ApiError, ApiErrorKind};
use herdr_agentsview::app::{
    App, AppCommand, BreakdownCategory, BreakdownValue, CompactRegion, Focus, InputKey, Loadable,
    MetadataKind, SessionSortColumn, SortDirection,
};
use herdr_agentsview::wire::{Automation, ProjectInfo, ReportSelection};

#[path = "support/activity.rs"]
mod activity_support;

use activity_support::{ready_app, selection};

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 9).unwrap()
}

fn report_selection(command: &Option<AppCommand>) -> &ReportSelection {
    match command {
        Some(AppCommand::FetchReport(selection)) => selection,
        other => panic!("expected one report request, got {other:?}"),
    }
}

#[test]
fn tab_and_backtab_follow_visual_focus_order() {
    // If focus order diverges from visual order, keyboard navigation becomes unpredictable
    // even though every control remains technically reachable.
    let mut app = App::new(selection(), Duration::from_secs(300));
    let expected = [
        Focus::Date,
        Focus::Project,
        Focus::Agent,
        Focus::Machine,
        Focus::Automation,
        Focus::Timeline,
        Focus::Sessions,
        Focus::Breakdowns,
        Focus::Date,
    ];

    assert_eq!(app.focus(), expected[0]);
    for focus in &expected[1..] {
        app.handle_input(InputKey::Tab, today());
        assert_eq!(app.focus(), *focus);
    }
    app.handle_input(InputKey::BackTab, today());
    assert_eq!(app.focus(), Focus::Breakdowns);
}

#[test]
fn focus_navigation_keeps_the_compact_data_region_visible() {
    // If focus and compact-region state diverge, the footer advertises controls for a
    // Sessions or Breakdown region that is not on screen.
    let mut app = ready_app();
    app.set_focus(Focus::Timeline);

    app.handle_input(InputKey::Tab, today());
    assert_eq!(app.focus(), Focus::Sessions);
    assert_eq!(app.compact_region(), CompactRegion::Sessions);

    app.handle_input(InputKey::Tab, today());
    assert_eq!(app.focus(), Focus::Breakdowns);
    assert_eq!(app.compact_region(), CompactRegion::Breakdown);

    app.handle_input(InputKey::BackTab, today());
    assert_eq!(app.focus(), Focus::Sessions);
    assert_eq!(app.compact_region(), CompactRegion::Sessions);
}

#[test]
fn date_keys_move_calendar_days_and_backspace_restores_today() {
    // If date input uses duration arithmetic or resets silently, DST-adjacent calendar
    // selections and the visible filter can drift.
    let mut app = App::new(selection(), Duration::from_secs(300));

    let command = app.handle_input(InputKey::Left, today());
    assert_eq!(
        app.selection().date,
        NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()
    );
    assert_eq!(
        report_selection(&command).date,
        NaiveDate::from_ymd_opt(2026, 8, 7).unwrap()
    );

    let command = app.handle_input(InputKey::Backspace, today());
    assert_eq!(app.selection().date, today());
    assert_eq!(report_selection(&command).date, today());

    let command = app.handle_input(InputKey::Right, today());
    assert!(command.is_none());
    assert_eq!(app.selection().date, today());
}

#[test]
fn t_jumps_to_today_from_any_focus_and_requests_a_report() {
    // If jump-to-today is scoped to the Date selector or forgets the fetch, the operator
    // lands on a stale date or stares at yesterday's report labeled as today's.
    let mut app = ready_app();
    app.set_focus(Focus::Sessions);
    assert!(app
        .contextual_keys()
        .iter()
        .any(|hint| hint.key == "t" && hint.action == "today"));

    let command = app.handle_input(InputKey::Char('t'), today());

    assert_eq!(app.selection().date, today());
    assert_eq!(report_selection(&command), app.selection());
}

#[test]
fn t_on_today_does_not_request_a_redundant_report() {
    // If jumping to the already selected day still fetches, the key silently discards the
    // rendered report and session selection for an identical request.
    let mut app = ready_app();
    app.handle_input(InputKey::Char('t'), today());

    let command = app.handle_input(InputKey::Char('t'), today());

    assert!(command.is_none());
    assert_eq!(app.selection().date, today());
}

#[test]
fn project_search_keeps_t_as_text_instead_of_jumping_the_date() {
    // If the global jump fires while the project search owns character input, typing a
    // project name containing `t` rewrites the date filter and launches a report request.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "project-alpha".to_owned(),
        session_count: 1,
    }]));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, today());

    let command = app.handle_input(InputKey::Char('t'), today());

    assert!(command.is_none());
    assert_eq!(app.popup().unwrap().query, "t");
    assert_eq!(
        app.selection().date,
        NaiveDate::from_ymd_opt(2026, 8, 8).unwrap()
    );
}

#[test]
fn project_popup_accepts_metadata_and_escape_cancels() {
    // If popup acceptance reads display text rather than typed metadata, All and a real
    // similarly named project can be confused or Esc can still mutate the filter.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "project-alpha".to_owned(),
        session_count: 2,
    }]));
    app.set_focus(Focus::Project);

    assert!(app.handle_input(InputKey::Enter, today()).is_none());
    assert_eq!(
        app.popup().unwrap().labels().collect::<Vec<_>>(),
        ["All", "project-alpha"]
    );
    app.handle_input(InputKey::Down, today());
    let command = app.handle_input(InputKey::Enter, today());
    assert_eq!(app.selection().project.as_deref(), Some("project-alpha"));
    assert_eq!(report_selection(&command), app.selection());

    app.handle_input(InputKey::Enter, today());
    app.handle_input(InputKey::Up, today());
    app.handle_input(InputKey::Escape, today());
    assert_eq!(app.selection().project.as_deref(), Some("project-alpha"));
    assert!(app.popup().is_none());
}

#[test]
fn project_popup_fuzzy_filters_while_preserving_text_input() {
    // If the Project picker only supports row navigation, large project lists remain slow;
    // if `q` is still treated as global quit while searching, valid queries close the app.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![
        ProjectInfo {
            name: "agentsview-herdr-dashboard".to_owned(),
            session_count: 3,
        },
        ProjectInfo {
            name: "herdr-core".to_owned(),
            session_count: 2,
        },
        ProjectInfo {
            name: "unrelated".to_owned(),
            session_count: 1,
        },
    ]));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, today());

    for key in ['a', 'v', 'd'] {
        assert!(app.handle_input(InputKey::Char(key), today()).is_none());
    }
    let popup = app.popup().unwrap();
    assert_eq!(popup.query, "avd");
    assert_eq!(
        popup.labels().collect::<Vec<_>>(),
        ["agentsview-herdr-dashboard"]
    );

    app.handle_input(InputKey::Backspace, today());
    assert_eq!(app.popup().unwrap().query, "av");
    app.handle_input(InputKey::Char('q'), today());
    assert_eq!(app.popup().unwrap().query, "avq");
    assert!(app.handle_input(InputKey::Backspace, today()).is_none());

    let command = app.handle_input(InputKey::Enter, today());
    assert_eq!(
        app.selection().project.as_deref(),
        Some("agentsview-herdr-dashboard")
    );
    assert_eq!(report_selection(&command), app.selection());
}

#[test]
fn clearing_a_project_query_restores_the_applied_project() {
    // If erasing a fuzzy query resets the highlight to All, pressing Enter clears an
    // existing project filter and launches a broader report unexpectedly.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![
        ProjectInfo {
            name: "agentsview-herdr-dashboard".to_owned(),
            session_count: 3,
        },
        ProjectInfo {
            name: "herdr-core".to_owned(),
            session_count: 2,
        },
    ]));
    app.set_project(Some("herdr-core".to_owned()));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, today());

    app.handle_input(InputKey::Char('h'), today());
    app.handle_input(InputKey::Backspace, today());

    assert_eq!(app.popup().unwrap().selected, 2);
    assert!(app.handle_input(InputKey::Enter, today()).is_none());
    assert_eq!(app.selection().project.as_deref(), Some("herdr-core"));
}

#[test]
fn project_search_ranks_the_stronger_match_and_keeps_no_match_open() {
    // If fuzzy results are ordered backwards, Enter applies a weaker project match; if an
    // empty result closes on Enter, the operator loses the query without making a choice.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![
        ProjectInfo {
            name: "agentsview-herdr-dashboard".to_owned(),
            session_count: 3,
        },
        ProjectInfo {
            name: "herdr-core".to_owned(),
            session_count: 2,
        },
    ]));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, today());
    for key in "herdr".chars() {
        app.handle_input(InputKey::Char(key), today());
    }

    assert_eq!(app.popup().unwrap().labels().next(), Some("herdr-core"));
    app.handle_input(InputKey::Escape, today());
    app.handle_input(InputKey::Enter, today());
    for key in "zzz".chars() {
        app.handle_input(InputKey::Char(key), today());
    }

    assert!(app.popup().unwrap().is_empty());
    assert!(app.handle_input(InputKey::Enter, today()).is_none());
    assert!(app.popup().is_some());
    assert!(app.selection().project.is_none());
}

#[test]
fn popup_hints_expose_the_still_active_global_quit_key() {
    // If modal hints omit a global action that remains active, the popup makes `q` a
    // destructive surprise instead of a discoverable close-pane command.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_focus(Focus::Automation);
    app.handle_input(InputKey::Enter, today());

    assert!(app.popup().is_some());
    assert!(app
        .contextual_keys()
        .iter()
        .any(|hint| hint.key == "q" && hint.action == "close dashboard"));
    assert_eq!(
        app.handle_input(InputKey::Char('q'), today()),
        Some(AppCommand::Quit)
    );
    assert_eq!(
        app.handle_input(InputKey::Quit, today()),
        Some(AppCommand::Quit)
    );
}

#[test]
fn project_named_all_remains_distinct_from_the_reset_choice() {
    // If popup selection is recovered from display labels, a real project named All
    // resolves to the synthetic reset row and Enter unexpectedly broadens the report.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "All".to_owned(),
        session_count: 1,
    }]));
    app.set_project(Some("All".to_owned()));
    app.set_focus(Focus::Project);

    app.handle_input(InputKey::Enter, today());

    assert_eq!(
        app.popup().unwrap().labels().collect::<Vec<_>>(),
        ["All", "All"]
    );
    assert_eq!(app.popup().unwrap().selected, 1);
    assert!(app.handle_input(InputKey::Enter, today()).is_none());
    assert_eq!(app.selection().project.as_deref(), Some("All"));

    app.handle_input(InputKey::Enter, today());
    app.handle_input(InputKey::Up, today());
    let command = app.handle_input(InputKey::Enter, today());
    assert!(app.selection().project.is_none());
    assert_eq!(report_selection(&command), app.selection());
}

#[test]
fn backspace_clears_only_the_focused_filter() {
    // If clearing one selector resets the whole selection, keyboard recovery can erase
    // deliberate filters and request a much broader report.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_project(Some("project-alpha".to_owned()));
    app.set_agent(Some("codex".to_owned()));
    app.set_focus(Focus::Project);

    let command = app.handle_input(InputKey::Backspace, today());

    assert!(app.selection().project.is_none());
    assert_eq!(app.selection().agent.as_deref(), Some("codex"));
    assert_eq!(report_selection(&command), app.selection());
}

#[test]
fn automation_popup_uses_only_the_three_supported_categories() {
    // If browser-only timing categories leak into this selector, the REST request would use
    // values outside the official all/interactive/automated contract.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_focus(Focus::Automation);

    app.handle_input(InputKey::Enter, today());
    assert_eq!(
        app.popup().unwrap().labels().collect::<Vec<_>>(),
        ["All", "Interactive", "Automated"]
    );
    app.handle_input(InputKey::Down, today());
    let command = app.handle_input(InputKey::Enter, today());

    assert_eq!(app.selection().automation, Automation::Interactive);
    assert_eq!(report_selection(&command), app.selection());
}

#[test]
fn failed_metadata_retries_contextually_without_refreshing_the_report() {
    // If contextual retry emits a report request, a local selector failure causes needless
    // report traffic while leaving the failed metadata untouched.
    let mut app = ready_app();
    app.apply_projects(Err(ApiError {
        kind: ApiErrorKind::Network,
        message: "projects unavailable".to_owned(),
    }));
    app.set_focus(Focus::Project);

    let hints = app.contextual_keys();
    assert!(hints
        .iter()
        .any(|hint| hint.key == "r" && hint.action == "retry"));
    assert!(!hints.iter().any(|hint| hint.key == "Enter"));

    let command = app.handle_input(InputKey::Char('r'), today());

    assert_eq!(
        command,
        Some(AppCommand::FetchMetadata(MetadataKind::Projects))
    );
    assert!(matches!(app.projects(), Loadable::Loading));
}

#[test]
fn loading_metadata_does_not_advertise_an_inert_chooser() {
    // If a loading selector advertises Enter, the footer promises a popup while the input
    // handler correctly refuses to open one until metadata arrives.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_focus(Focus::Project);

    let hints = app.contextual_keys();

    assert!(!hints.iter().any(|hint| hint.key == "Enter"));
}

#[test]
fn in_flight_report_does_not_advertise_an_inert_refresh() {
    // If loading or refreshing keeps the global refresh hint, the footer promises a request
    // while the overlap guard correctly ignores the key.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();

    let hints = app.contextual_keys();

    assert!(!hints.iter().any(|hint| hint.key == "r"));
    assert!(app.handle_input(InputKey::Char('r'), today()).is_none());
}

#[test]
fn unavailable_timeline_does_not_advertise_an_inert_slice() {
    // If initial loading advertises Enter, the footer promises session slicing while the
    // input handler correctly leaves timeline inspection disabled without report buckets.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.set_focus(Focus::Timeline);

    let hints = app.contextual_keys();

    assert!(!hints.iter().any(|hint| hint.key == "Enter"));
    app.handle_input(InputKey::Enter, today());
    assert!(!app.timeline_inspection_active());
}

#[test]
fn report_without_a_page_id_keeps_timeline_slicing_unavailable() {
    // If the valid v6 fallback without report_id advertises slicing, Enter replaces a populated
    // session table with an empty slice that no server request can ever fill.
    let mut value = activity_support::report();
    value.report_id = None;
    let expected_sessions = value.by_session.len();
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(Ok(Box::new(value)), Utc::now());
    app.set_focus(Focus::Timeline);

    let hints = app.contextual_keys();
    let command = app.handle_input(InputKey::Enter, today());

    assert!(!hints.iter().any(|hint| hint.key == "Enter"));
    assert!(command.is_none());
    assert!(!app.timeline_inspection_active());
    assert_eq!(app.sorted_sessions().len(), expected_sessions);
}

#[test]
fn stale_report_advertises_retry_consistently() {
    // If the stale header and footer name different actions for the same key, recovery copy is
    // internally contradictory even though both paths issue the same request.
    let mut app = ready_app();
    app.begin_refresh().unwrap();
    app.apply_report(
        Err(ApiError {
            kind: ApiErrorKind::Network,
            message: "offline".to_owned(),
        }),
        Utc::now(),
    );

    let hints = app.contextual_keys();

    assert!(hints
        .iter()
        .any(|hint| hint.key == "r" && hint.action == "retry"));
}

#[test]
fn session_keys_select_sort_column_direction_and_row() {
    // If horizontal and vertical session controls share state, choosing a sort column can
    // unexpectedly move the selected row or invert the wrong column.
    let mut app = ready_app();
    app.set_focus(Focus::Sessions);

    app.handle_input(InputKey::Right, today());
    assert_eq!(app.sort_column(), SessionSortColumn::Cost);
    assert_eq!(app.session_cursor(), 0);
    app.handle_input(InputKey::Enter, today());
    assert_eq!(app.sort_direction(), SortDirection::Ascending);
    assert_eq!(
        app.sorted_sessions()[app.session_cursor()].session_id,
        "session-alpha"
    );
    app.handle_input(InputKey::Char('k'), today());
    assert_eq!(app.session_cursor(), 1);
    assert_eq!(
        app.sorted_sessions()[app.session_cursor()].session_id,
        "session-beta"
    );
}

#[test]
fn timeline_and_breakdown_keys_change_only_the_focused_region() {
    // If region keys are not focus-scoped, inspecting a timeline bucket can alter the
    // breakdown category or selected session off-screen.
    let mut app = ready_app();
    app.set_focus(Focus::Timeline);
    assert!(!app.timeline_inspection_active());
    assert_eq!(app.timeline_cursor(), 0);

    app.handle_input(InputKey::Right, today());
    assert_eq!(app.timeline_cursor(), 0);

    let command = app.handle_input(InputKey::Enter, today());
    assert!(matches!(
        command,
        Some(AppCommand::FetchSessionPage(request))
            if request.report_id == "fixture-report-id" && request.bucket == 0
    ));
    assert!(app.timeline_inspection_active());
    let command = app.handle_input(InputKey::Right, today());
    assert!(matches!(
        command,
        Some(AppCommand::FetchSessionPage(request)) if request.bucket == 1
    ));
    assert_eq!(app.timeline_cursor(), 1);
    assert_eq!(app.breakdown_category(), BreakdownCategory::Project);

    assert_eq!(
        app.handle_input(InputKey::Enter, today()),
        Some(AppCommand::CancelSessionPage)
    );
    assert!(!app.timeline_inspection_active());
    app.handle_input(InputKey::Right, today());
    assert_eq!(app.timeline_cursor(), 1);
    assert!(matches!(
        app.handle_input(InputKey::Enter, today()),
        Some(AppCommand::FetchSessionPage(request)) if request.bucket == 1
    ));
    assert!(app.timeline_inspection_active());
    assert_eq!(app.timeline_cursor(), 1);

    app.set_focus(Focus::Breakdowns);
    app.handle_input(InputKey::Right, today());
    app.handle_input(InputKey::Char('v'), today());
    assert_eq!(app.breakdown_category(), BreakdownCategory::Model);
    assert_eq!(app.breakdown_value(), BreakdownValue::Cost);
    assert_eq!(app.timeline_cursor(), 1);
}

#[test]
fn compact_region_keys_remember_the_last_breakdown_category() {
    // If compact switching forgets category state, `b` cannot return users to the data they
    // were comparing before viewing Sessions.
    let mut app = ready_app();
    assert_eq!(app.compact_region(), CompactRegion::Sessions);

    app.handle_input(InputKey::Char('b'), today());
    assert_eq!(app.compact_region(), CompactRegion::Breakdown);
    assert_eq!(app.breakdown_category(), BreakdownCategory::Project);
    app.handle_input(InputKey::Char('m'), today());
    assert_eq!(app.breakdown_category(), BreakdownCategory::Model);
    app.handle_input(InputKey::Char('s'), today());
    assert_eq!(app.compact_region(), CompactRegion::Sessions);
    app.handle_input(InputKey::Char('b'), today());
    assert_eq!(app.breakdown_category(), BreakdownCategory::Model);
    app.handle_input(InputKey::Char('a'), today());
    assert_eq!(app.breakdown_category(), BreakdownCategory::Agent);
    app.handle_input(InputKey::Char('p'), today());
    assert_eq!(app.breakdown_category(), BreakdownCategory::Project);
}

#[test]
fn refresh_help_and_quit_emit_contextual_behavior() {
    // If global keys bypass current state, refreshes can overlap and help can advertise
    // controls for the wrong region.
    let mut app = ready_app();

    let refresh = app.handle_input(InputKey::Char('r'), today());
    assert_eq!(report_selection(&refresh), app.selection());
    assert!(app.handle_input(InputKey::Char('r'), today()).is_none());

    app.handle_input(InputKey::Char('?'), today());
    assert!(app.help_open());
    assert!(app.contextual_keys().iter().any(|hint| hint.key == "Esc"));
    app.handle_input(InputKey::Escape, today());
    assert!(!app.help_open());

    assert_eq!(
        app.handle_input(InputKey::Char('q'), today()),
        Some(AppCommand::Quit)
    );
}

#[test]
fn retry_without_a_report_starts_a_foreground_load() {
    // If manual retry requires last-good data, initial network failures become terminal
    // until the pane is restarted.
    let mut app = App::new(selection(), Duration::from_secs(300));
    app.begin_foreground_load();
    app.apply_report(
        Err(ApiError {
            kind: ApiErrorKind::Network,
            message: "offline".to_owned(),
        }),
        Utc::now(),
    );

    let command = app.handle_input(InputKey::Char('r'), today());

    assert_eq!(report_selection(&command), app.selection());
}
