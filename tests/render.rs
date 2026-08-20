// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use herdr_agentsview::api::{ApiError, ApiErrorKind};
use herdr_agentsview::app::{App, BreakdownValue, Focus, InputKey};
use herdr_agentsview::render::{self, ColorMode, LayoutClass, TerminalCapabilities};
use herdr_agentsview::wire::{Automation, Money, ProjectInfo};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

#[path = "support/activity.rs"]
// This shared fixture module also exposes helpers used by the app/input integration targets.
#[allow(dead_code)]
mod activity;
#[path = "support/render.rs"]
mod render_support;

use render_support::*;

#[test]
fn wide_ready_render_preserves_the_complete_operations_stack() {
    // If the wide topology drops a region or summary fact, operators lose the full
    // scan path even though the report still contains the data.
    let app = ready_app(ColorMode::Color);
    let text = render::to_text_at(&app, 200, 50, render_time());

    assert_golden("golden/ready-200x50.txt", &text);
    let lowercase = text.to_lowercase();
    for label in [
        "Agents",
        "Time",
        "Work",
        "Sessions",
        "Projects",
        "Models",
        "Interactive",
        "Automated",
        "Untimed",
        "Concurrent agents",
        "Window",
        "Breakdowns",
    ] {
        assert!(
            lowercase.contains(&label.to_lowercase()),
            "missing {label:?}\n{text}"
        );
    }
    assert_width(&text, 200);
}

#[test]
fn medium_ready_render_compacts_without_changing_information_order() {
    // If medium rendering hides facts instead of tightening the same stack, resize changes
    // what the dashboard means rather than only how densely it reads.
    let app = ready_app(ColorMode::Color);
    let text = render::to_text_at(&app, 120, 40, render_time());

    assert_golden("golden/ready-120x40.txt", &text);
    assert_before(&text, "┌ Summary", "┌ Concurrent agents");
    assert_before(&text, "┌ Concurrent agents", "┌ Sessions");
    assert_before(&text, "┌ Sessions", "┌ Breakdowns");
    for category in ["Project", "Model", "Agent"] {
        assert!(text.contains(category));
    }
    assert_width(&text, 120);
}

#[test]
fn compact_ready_render_keeps_overview_and_switchable_data_region() {
    // If compact mode merely squeezes the desktop table, required fields become unreadable
    // and the lower-region keyboard rail has no visible effect.
    let app = ready_app(ColorMode::Color);
    let text = render::to_text_at(&app, 80, 24, render_time());

    assert_golden("golden/ready-80x24.txt", &text);
    for label in [
        "Agents",
        "Time",
        "Work",
        "Projects",
        "▲",
        "●",
        "○",
        "◷",
        "Concurrent agents",
        "Sessions",
        "Sections",
    ] {
        assert!(text.contains(label), "missing {label:?}\n{text}");
    }
    for detail in ["Model:", "Agent:", "Window:"] {
        assert!(text.contains(detail), "missing {detail:?}\n{text}");
    }
    assert!(!text.contains("Breakdowns (all)"));
    assert_width(&text, 80);
}

#[test]
fn compact_filters_keep_every_label_without_a_text_focus_marker() {
    // If selected metadata values have no individual width budget, an early long value can
    // push later filters and the active Session marker outside an 80-column pane.
    let selection = activity::selection()
        .with_project("project-with-a-very-long-operator-facing-name")
        .with_agent("agent-with-a-very-long-operator-facing-name")
        .with_machine("machine-with-a-very-long-operator-facing-name")
        .with_automation(Automation::Interactive);
    let mut app = App::new(selection, Duration::from_secs(300));
    app.set_color_mode(ColorMode::Monochrome);
    app.set_focus(Focus::Automation);

    let text = render::to_text_at(&app, 80, 24, render_time());
    let filters = text.lines().nth(1).expect("header has a filter row");

    for label in ["Date", "Proj", "Agent", "Host", "Session Interactive"] {
        assert!(filters.contains(label), "missing {label:?}\n{filters}");
    }
    assert!(!filters.contains(">Session"), "{filters}");
    assert_eq!(UnicodeWidthStr::width(filters), 80);
}

#[test]
fn medium_and_wide_filters_budget_long_metadata_values() {
    // If only compact layouts budget metadata, a long project or agent can still push Machine,
    // Session, and the active focus marker out of the 120- or 200-column header.
    let selection = activity::selection()
        .with_project("project-with-an-extremely-long-operator-facing-name-that-keeps-going")
        .with_agent("agent-with-an-extremely-long-operator-facing-name-that-keeps-going")
        .with_machine("machine-with-an-extremely-long-operator-facing-name-that-keeps-going")
        .with_automation(Automation::Interactive);
    let mut app = App::new(selection, Duration::from_secs(300));
    app.set_color_mode(ColorMode::Monochrome);
    app.set_focus(Focus::Automation);

    for (width, height) in [(120, 40), (200, 50)] {
        let text = render::to_text_at(&app, width, height, render_time());
        let filters = text.lines().nth(1).expect("header has a filter row");

        for label in ["Date", "Project", "Agent", "Machine", "Session Interactive"] {
            assert!(filters.contains(label), "missing {label:?}\n{filters}");
        }
        assert!(!filters.contains(">Session"), "{filters}");
        assert_eq!(UnicodeWidthStr::width(filters), usize::from(width));
    }
}

#[test]
fn compact_summary_keeps_semantic_groups_under_large_values() {
    // If large values erase the group labels, the compact summary falls back to an
    // undifferentiated run of numbers with no stable scan path.
    let mut report = activity::report();
    report.peak.agents = usize::MAX;
    report.totals.active_minutes = 12_345_678.0;
    report.totals.idle_minutes = 12_345_678.0;
    report.totals.agent_minutes = 12_345_678.0;
    report.totals.sessions = usize::MAX;
    report.totals.interactive_sessions = usize::MAX;
    report.totals.automated_sessions = usize::MAX;
    report.totals.untimed_sessions = usize::MAX;
    report.totals.distinct_projects = usize::MAX;
    report.totals.distinct_models = usize::MAX;
    report.totals.cost = Money {
        microdollars: i64::MAX,
    };
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 80, 24, render_time());
    let summary = text
        .lines()
        .skip_while(|line| !line.contains("┌ Summary"))
        .skip(1)
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");

    for label in ["Agents", "Time", "Work", "Sessions", "Projects"] {
        assert!(summary.contains(label), "missing {label:?}\n{summary}");
    }
}

#[test]
fn compact_summary_leads_the_session_group_with_its_total() {
    // If the session breakdown precedes the total, large class counts can hide the primary
    // fact before the terminal clips the rest of the compact row.
    let mut report = activity::report();
    report.totals.sessions = usize::MAX;
    report.totals.interactive_sessions = 900;
    report.totals.automated_sessions = 300;
    report.totals.untimed_sessions = 34;
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 80, 24, render_time());
    let sessions_row = text
        .lines()
        .find(|line| line.contains("Sessions  "))
        .expect("compact summary has a Sessions row");

    assert!(
        sessions_row.contains("Sessions  18,446,744,073,709,551,615 ="),
        "{sessions_row}"
    );
}

#[test]
fn medium_summary_groups_each_metric_family_into_a_column() {
    // If summary values are spread independently, dividers create a grid without giving
    // agents, time, work, sessions, and projects a coherent relationship.
    let app = ready_app(ColorMode::Monochrome);
    let text = render::to_text_at(&app, 120, 40, render_time());
    let summary = text
        .lines()
        .skip_while(|line| !line.contains("┌ Summary"))
        .skip(1)
        .take(2)
        .collect::<Vec<_>>();

    for label in ["Agents", "Time", "Work", "Sessions", "Projects"] {
        assert!(summary[0].contains(label), "missing {label:?}\n{text}");
    }
    assert!(summary[1].contains("▲ 7 @"), "{text}");
    assert!(summary[1].contains("● 25m"), "{text}");
    assert!(summary[1].contains("○ 5m"), "{text}");
    assert!(summary[1].contains("◷ 30 / $6.50"), "{text}");
    assert!(summary[1].contains("3 ="), "{text}");
    assert!(summary[1].contains("int"), "{text}");
    assert!(summary[1].contains("auto"), "{text}");
    assert!(summary[1].contains("unt"), "{text}");
    assert!(summary[1].contains("3 · 3 models"), "{text}");
}

#[test]
fn activity_counts_are_named_as_concurrent_agents() {
    // If the dashboard uses the abstract noun alone, operators have to infer what is
    // concurrent instead of reading the unit directly from the summary and chart.
    let app = ready_app(ColorMode::Monochrome);
    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("Concurrent agents"), "{text}");
    assert!(!text.contains("Concurrency"), "{text}");
}

#[test]
fn compact_sessions_names_hidden_sort_columns() {
    // If compact mode sorts by Model, Agent, or Window without naming that state, row order
    // changes while every visible table header appears unsorted.
    let mut app = ready_app(ColorMode::Monochrome);
    app.set_focus(Focus::Sessions);
    for _ in 0..4 {
        app.handle_input(InputKey::Right, activity::selection().date);
    }
    app.toggle_timeline_inspection();

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert!(text.contains("Sort: Model↓"), "{text}");
}

#[test]
fn project_picker_renders_a_no_match_state() {
    // If a fuzzy query has no results but the menu renders as a blank box, operators cannot
    // distinguish a completed search from a drawing failure.
    let mut app = ready_app(ColorMode::Monochrome);
    app.apply_projects(Ok(vec![ProjectInfo {
        name: "project-alpha".to_owned(),
        session_count: 1,
    }]));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, activity::selection().date);
    for key in "zzz".chars() {
        app.handle_input(InputKey::Char(key), activity::selection().date);
    }

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert!(text.contains("No matching projects"), "{text}");
}

#[test]
fn compact_session_details_budget_each_hidden_field() {
    // If selected-row details clip one concatenated string, a long model or agent can erase
    // the later Agent and Window fields entirely.
    let mut report = activity::report();
    report.by_session[0].primary_model = "model-with-a-name-that-is-far-too-long".repeat(3);
    report.by_session[0].agent = "agent-with-an-equally-long-name".repeat(3);
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 80, 24, render_time());

    for label in ["Model:", "Agent:", "Window: 13:00–13:20"] {
        assert!(text.contains(label), "missing {label:?}\n{text}");
    }
}

#[test]
fn compact_session_details_give_unused_field_space_to_its_peer() {
    // If hidden fields keep fixed half-widths, a short value leaves blank capacity while its
    // longer peer is needlessly clipped in the same detail row.
    for (model, agent, expected) in [
        (
            "m",
            "agent-abcdefghijklmnopqrstuvwxyz0123456789",
            "Agent: agent-abcdefghijklmnopqrst",
        ),
        (
            "model-abcdefghijklmnopqrstuvwxyz0123456789",
            "a",
            "Model: model-abcdefghijklmnopqrst",
        ),
    ] {
        let mut report = activity::report();
        report.by_session[0].primary_model = model.to_owned();
        report.by_session[0].agent = agent.to_owned();
        let app = app_with_report(report, ColorMode::Monochrome);

        let text = render::to_text_at(&app, 80, 24, render_time());

        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn text_render_keeps_the_selected_session_inside_its_target_viewport() {
    // If pure rendering trusts scroll state computed for another pane height, snapshots and
    // diagnostics can hide the selected row even though the interactive renderer would sync it.
    let mut report = activity::report();
    let template = report.by_session[0].clone();
    report.by_session = (0..30)
        .map(|index| {
            let mut row = template.clone();
            row.session_id = format!("session-{index:02}");
            row.title = format!("session-{index:02}");
            row
        })
        .collect();
    report.totals.sessions = report.by_session.len();
    report.sessions_total = report.by_session.len();
    let mut app = app_with_report(report, ColorMode::Monochrome);
    app.set_focus(Focus::Sessions);
    app.move_session(29, 30);

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert!(text.contains("> session-29"), "{text}");
}

#[test]
fn compact_footer_retains_the_focused_region_keys() {
    // If compact rendering falls back to global keys only, operators can focus the
    // timeline without seeing how to activate its session slice.
    let mut app = ready_app(ColorMode::Monochrome);
    app.set_focus(Focus::Timeline);

    let text = render::to_text_at(&app, 80, 24, render_time());

    for label in ["Enter", "slice", "q", "quit"] {
        assert!(text.contains(label), "missing {label:?}\n{text}");
    }
    assert_width(&text, 80);
}

#[test]
fn timeline_axis_places_intermediate_clock_ticks() {
    // If a full-day axis renders only matching midnight endpoints, operators cannot place
    // concurrency spikes in the day without moving focus bucket by bucket.
    let app = ready_app(ColorMode::Monochrome);

    let text = render::to_text_at(&app, 120, 40, render_time());

    let axis = text
        .lines()
        .find(|line| line.contains("06:00") && line.contains("12:00") && line.contains("18:00"))
        .unwrap_or_else(|| panic!("missing quarter-day timeline ticks\n{text}"));
    assert_eq!(axis.matches("00:00").count(), 2, "{axis}");
}

#[test]
fn y_axis_reserves_plot_width_and_keeps_time_axis_aligned() {
    // If the y-axis is painted over the existing chart width, the first time tick sits under
    // the labels instead of the data; if the x-axis keeps the old width, its right edge clips.
    for (width, height, scale) in [(80, 24, "0–7│"), (120, 40, "7 ┤"), (200, 50, "7 ┤")] {
        let app = ready_app(ColorMode::Monochrome);
        let text = render::to_text_at(&app, width, height, render_time());
        let scale_line = text
            .lines()
            .find(|line| line.contains(scale))
            .unwrap_or_else(|| panic!("missing y-axis scale {scale:?}\n{text}"));
        let scale_end = scale_line.find(scale).unwrap() + scale.len();
        let plot_start = UnicodeWidthStr::width(&scale_line[..scale_end]);
        let time_axis = text
            .lines()
            .find(|line| line.contains("00:00") && line.contains("12:00"))
            .unwrap_or_else(|| panic!("missing timeline axis\n{text}"));
        let first_tick = time_axis.find("00:00").unwrap();
        let last_tick = time_axis.rfind("00:00").unwrap();

        assert_eq!(UnicodeWidthStr::width(&time_axis[..first_tick]), plot_start);
        assert_eq!(
            UnicodeWidthStr::width(&time_axis[..last_tick]) + 5,
            usize::from(width) - 1
        );
    }
}

#[test]
fn compact_partial_timeline_marks_its_observed_cutoff() {
    // If the compact chart has only one plot row, the timestamp replaces the cutoff bar
    // instead of labeling it from above.
    let app = stale_partial_app(ColorMode::Monochrome);

    let text = render::to_text_at(&app, 80, 24, render_time());

    let lines = text.lines().collect::<Vec<_>>();
    let label_row = lines
        .iter()
        .rposition(|line| line.contains("16:30"))
        .unwrap_or_else(|| panic!("missing compact observation cutoff time\n{text}"));
    let marker_column = lines[label_row]
        .chars()
        .position(|character| character == ':')
        .expect("cutoff time has a center character");
    assert_eq!(
        lines[label_row + 1].chars().nth(marker_column),
        Some('│'),
        "cutoff marker is not centered below its time\n{text}"
    );
    text.lines()
        .find(|line| line.matches("00:00").count() == 2)
        .unwrap_or_else(|| panic!("missing compact timeline axis\n{text}"));
}

#[test]
fn partial_timeline_marks_the_future_inside_a_straddling_bucket() {
    // If elapsed bucket count paints each observed bucket in full, a partial final bucket can
    // draw activity beyond effective_end even while the axis names that region as future.
    let report = activity::report();
    let total = report
        .range_end
        .signed_duration_since(report.range_start)
        .num_milliseconds();
    let elapsed = report
        .effective_end
        .signed_duration_since(report.range_start)
        .num_milliseconds();
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 120, 40, render_time());

    let future_line = text
        .lines()
        .find(|line| line.matches('┄').count() > 1)
        .unwrap_or_else(|| panic!("missing sub-bucket future region\n{text}"));
    let cells = future_line.chars().collect::<Vec<_>>();
    let plot_start = cells
        .iter()
        .position(|cell| *cell == '┼')
        .expect("missing y-axis origin")
        + 1;
    let cutoff = cells
        .windows(2)
        .position(|pair| pair == ['│', '┄'])
        .expect("missing cutoff marker before future region");
    let plot_width = cells.len() - 1 - plot_start;
    let expected_cutoff =
        (usize::try_from(elapsed).unwrap() * plot_width).div_ceil(usize::try_from(total).unwrap());

    assert_eq!(cutoff - plot_start, expected_cutoff, "{future_line}");
    assert!(
        cells[cutoff + 1..cells.len() - 1]
            .iter()
            .all(|cell| *cell == '┄'),
        "{future_line}"
    );
}

#[test]
fn compact_breakdown_switch_replaces_the_lower_region_and_preserves_value_mode() {
    // If compact switching only changes the rail, users lose the sole lower-region slot or
    // see a Cost label backed by Agent-min values.
    let mut app = ready_app(ColorMode::Monochrome);
    app.handle_input(InputKey::Char('b'), activity::selection().date);
    app.handle_input(InputKey::Char('v'), activity::selection().date);

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert_eq!(app.breakdown_value(), BreakdownValue::Cost);
    assert!(text.contains("Breakdown · Project · Cost"));
    assert!(text.contains("project-alpha"));
    assert!(text.contains("$3.00"));
    assert!(!text.contains("┌ Sessions"));
    assert_width(&text, 80);
}

#[test]
fn wide_breakdown_divider_preserves_the_selected_category_marker() {
    // If a column divider overwrites the selected heading marker, Right changes the
    // breakdown category without a visible text cue in non-color terminals.
    let mut app = ready_app(ColorMode::Monochrome);
    app.set_focus(Focus::Breakdowns);
    app.handle_input(InputKey::Right, activity::selection().date);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("│>Model"), "{text}");
    assert_width(&text, 120);
}

#[test]
fn monochrome_render_keeps_interactive_automated_focus_and_stacking_semantics() {
    // If color is the only differentiator, an operator in a low-capability terminal cannot
    // distinguish the two concurrency sources or the focused region.
    let app = ready_app(ColorMode::Monochrome);
    let text = render::to_text_at(&app, 120, 40, render_time());

    assert_golden("golden/monochrome-120x40.txt", &text);
    assert!(text.contains("I Interactive"));
    assert!(text.contains("A Automated"));
    assert!(text.contains('█'));
    assert!(text.contains('▓'));
    assert_width(&text, 120);
}

#[test]
fn focused_timeline_names_the_inspected_bucket_and_concurrency_split() {
    // If selected-bucket rendering disappears, Right and Left still mutate state but give
    // operators no visible interval or source counts for the bucket they are inspecting.
    let mut app = ready_app(ColorMode::Monochrome);
    app.set_focus(Focus::Timeline);
    app.toggle_timeline_inspection();
    app.move_timeline(3);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("05:15-06:00"), "{text}");
    assert!(text.contains("Interactive 4"), "{text}");
    assert!(text.contains("Automated 2"), "{text}");
    assert_width(&text, 120);
}

#[test]
fn focused_partial_timeline_clamps_the_last_observed_interval() {
    // If the final elapsed bucket keeps its nominal end, the inspector claims activity was
    // observed after effective_end even though the chart correctly marks that span as future.
    let mut app = app_with_report(activity::report(), ColorMode::Monochrome);
    app.set_focus(Focus::Timeline);
    app.toggle_timeline_inspection();
    app.move_timeline(1);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("13:15-13:20"), "{text}");
    assert!(!text.contains("13:15-13:30  Interactive"), "{text}");
}

#[test]
fn focused_partial_timeline_names_unobserved_buckets_as_future() {
    // If a wholly unobserved bucket is shown as zero concurrency, users cannot distinguish
    // missing future data from an observed quiet period while inspecting the chart.
    let mut report = activity::report();
    report.effective_end = report.buckets[0].end;
    report.as_of = Some(report.effective_end);
    report.elapsed_bucket_count = 1;
    let mut app = app_with_report(report, ColorMode::Monochrome);
    app.set_focus(Focus::Timeline);
    app.toggle_timeline_inspection();
    app.move_timeline(1);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("13:15-13:30  future"), "{text}");
    assert!(!text.contains("13:15-13:30  Interactive"), "{text}");
}

#[test]
fn focused_timeline_disambiguates_a_dst_fallback_bucket() {
    // If a repeated local hour is rendered without zone abbreviations, a bucket can appear
    // to run backward and two distinct instants receive the same clock label.
    let mut report = activity::report();
    report.timezone = "America/New_York".parse().unwrap();
    report.buckets.truncate(1);
    report.buckets[0].start = "2026-11-01T05:45:00Z".parse().unwrap();
    report.buckets[0].end = "2026-11-01T06:00:00Z".parse().unwrap();
    report.bucket_count = 1;
    report.elapsed_bucket_count = 1;
    let mut app = app_with_report(report, ColorMode::Monochrome);
    app.set_focus(Focus::Timeline);
    app.toggle_timeline_inspection();

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("01:45 EDT-01:00 EST"), "{text}");
}

#[test]
fn focused_timeline_disambiguates_intervals_inside_a_repeated_hour() {
    // If equal-offset endpoints skip the ambiguity check, two distinct buckets inside the
    // repeated hour can display the same interval even though one is EDT and the other EST.
    for (start, end, expected) in [
        (
            "2026-11-01T05:15:00Z",
            "2026-11-01T05:30:00Z",
            "01:15 EDT-01:30 EDT",
        ),
        (
            "2026-11-01T06:15:00Z",
            "2026-11-01T06:30:00Z",
            "01:15 EST-01:30 EST",
        ),
    ] {
        let mut report = activity::report();
        report.timezone = "America/New_York".parse().unwrap();
        report.buckets.truncate(1);
        report.buckets[0].start = start.parse().unwrap();
        report.buckets[0].end = end.parse().unwrap();
        report.bucket_count = 1;
        report.elapsed_bucket_count = 1;
        let mut app = app_with_report(report, ColorMode::Monochrome);
        app.set_focus(Focus::Timeline);
        app.toggle_timeline_inspection();

        let text = render::to_text_at(&app, 120, 40, render_time());

        assert!(text.contains(expected), "{text}");
    }
}

#[test]
fn repeated_hour_disambiguates_peak_and_observed_cutoff_clocks() {
    // If standalone clocks omit their zone during a repeated hour, peak and cutoff facts can
    // display the same local time while referring to different UTC instants.
    let mut report = activity::report();
    report.timezone = "America/New_York".parse().unwrap();
    report.peak.at = Some("2026-11-01T05:30:00Z".parse().unwrap());
    report.partial = true;
    report.effective_end = "2026-11-01T06:30:00Z".parse().unwrap();
    report.as_of = Some("2026-11-01T05:30:00Z".parse().unwrap());
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("▲ 2 @ 01:30 EDT"), "{text}");
    assert!(text.contains("01:30 EST│"), "{text}");
}

#[test]
fn loading_error_and_empty_states_never_render_invented_activity() {
    // If non-ready states pass through the normal zero renderer, loading and transport
    // failures look indistinguishable from a genuinely inactive day.
    let loading = render::to_text_at(&loading_app(ColorMode::Monochrome), 80, 24, render_time());
    let auth = render::to_text_at(
        &authentication_error_app(ColorMode::Monochrome),
        80,
        24,
        render_time(),
    );
    let empty = render::to_text_at(&empty_app(ColorMode::Monochrome), 80, 24, render_time());

    assert_golden("golden/loading-80x24.txt", &loading);
    assert_golden("golden/auth-error-80x24.txt", &auth);
    assert_golden("golden/empty-80x24.txt", &empty);
    assert!(loading.starts_with("AgentsView ⠖"), "{loading}");
    assert!(!loading.contains("▲ 0"));
    assert!(!loading.contains("Sessions (0 total)"));
    assert!(auth.contains("Authentication required"));
    assert!(auth.contains("runtime token"));
    assert!(!auth.contains("▲ 0"));
    assert!(!auth.contains("Sessions (0 total)"));
    assert!(empty.contains("No activity"));
    assert!(empty.contains("move date or clear filters"));
}

#[test]
fn initial_load_uses_one_animated_braille_spinner_for_the_whole_dashboard() {
    // If every empty region repeats the loading announcement, one request looks like five
    // independent operations and overwhelms the dashboard's information hierarchy.
    let app = loading_app(ColorMode::Monochrome);
    let first = render::to_text_at(&app, 120, 40, "1970-01-01T00:00:00Z".parse().unwrap());
    let second = render::to_text_at(&app, 120, 40, "1970-01-01T00:00:00.100Z".parse().unwrap());

    assert_eq!(first.matches('⠖').count(), 1, "{first}");
    assert_eq!(second.matches('⠲').count(), 1, "{second}");
    assert!(first.starts_with("AgentsView ⠖"), "{first}");
    assert_eq!(first.matches("loading Activity").count(), 0, "{first}");
    assert_ne!(first, second);
}

#[test]
fn refreshing_keeps_last_good_data_visible_with_freshness() {
    // If refresh rendering follows foreground-load rendering, periodic updates blank the
    // dashboard even though a coherent last-good report is still available.
    let mut app = ready_app(ColorMode::Monochrome);
    app.begin_refresh().unwrap();

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert!(text.starts_with("AgentsView "), "{text}");
    assert!(text.lines().next().unwrap().contains("Last update 12s ago"));
    assert!(text.contains("Implement activity view"));
    assert!(text.contains("▲ 7"));
}

#[test]
fn failure_kinds_render_distinct_recovery_copy_without_fake_totals() {
    // If boundary failures share generic copy, operators replace credentials for network
    // outages or retry incompatible schemas without seeing the actual action required.
    let cases = [
        (
            ApiErrorKind::Authentication,
            "AgentsView rejected the configured credential",
            "Credential rejected",
        ),
        (
            ApiErrorKind::Forbidden,
            "AgentsView access is forbidden",
            "Access forbidden",
        ),
        (
            ApiErrorKind::Timeout,
            "AgentsView stopped the Activity report at its server timeout",
            "stopped the Activity report",
        ),
        (
            ApiErrorKind::Network,
            "could not reach AgentsView",
            "Cannot reach AgentsView",
        ),
        (
            ApiErrorKind::Protocol,
            "unsupported Activity schema version 6",
            "Protocol mismatch",
        ),
        (
            ApiErrorKind::Server,
            "AgentsView server returned HTTP 500",
            "AgentsView server error",
        ),
    ];
    for (kind, message, expected) in cases {
        let app = failed_app(kind, message, ColorMode::Monochrome);
        let text = render::to_text_at(&app, 80, 24, render_time());
        assert!(text.contains(expected), "missing {expected:?}\n{text}");
        assert!(!text.contains("▲ 0"), "{text}");
    }
}

#[test]
fn compact_breakdown_keeps_error_and_empty_recovery_guidance() {
    // If compact mode shows state guidance only in Sessions, switching to Breakdown hides
    // the reason Activity is unavailable and the action that can recover an empty selection.
    let mut auth = authentication_error_app(ColorMode::Monochrome);
    auth.set_focus(Focus::Breakdowns);
    let auth = render::to_text_at(&auth, 80, 24, render_time());
    assert!(auth.contains("Authentication required"), "{auth}");
    assert!(auth.contains("runtime token"), "{auth}");

    let mut empty = empty_app(ColorMode::Monochrome);
    empty.set_focus(Focus::Breakdowns);
    let empty = render::to_text_at(&empty, 80, 24, render_time());
    assert!(empty.contains("No activity"), "{empty}");
    assert!(empty.contains("move date or clear filters"), "{empty}");
}

#[test]
fn stale_partial_render_distinguishes_observed_zeroes_from_future_buckets() {
    // If future buckets share the observed-zero glyph, a partial day claims inactivity that
    // the server has not yet measured.
    let app = stale_partial_app(ColorMode::Monochrome);
    let text = render::to_text_at(&app, 120, 40, "2026-08-08T17:31:12Z".parse().unwrap());

    assert_golden("golden/stale-partial-120x40.txt", &text);
    assert!(text.contains("Summary · through 16:30"), "{text}");
    assert!(text.contains("Stale 10m"));
    assert!(text.contains("request timed out"));
    assert!(text.contains("· observed zero"));
    assert!(text.contains('┄'));
    assert_width(&text, 120);
}

#[test]
fn metadata_failure_marks_only_its_filter() {
    // If metadata degradation is promoted to a page-level failure, one selector hides a
    // valid report and makes healthy filter lists look unavailable too.
    let app = metadata_degraded_app(ColorMode::Color);
    let text = render::to_text_at(&app, 120, 40, render_time());

    assert_golden("golden/metadata-degraded-120x40.txt", &text);
    assert!(text.contains("Project unavailable"));
    assert!(!text.contains("unavailable (r)"));
    assert!(text.contains("Agent All"));
    assert!(text.contains("Machine All"));
    assert!(text.contains("Sessions (3 total)"));
}

#[test]
fn long_stale_status_keeps_one_complete_activity_title() {
    // If header alignment round-trips through an ellipsized title string, a long stale
    // error duplicates a partial Activity label and steals cells from the status.
    let mut app = ready_app(ColorMode::Monochrome);
    app.begin_refresh().unwrap();
    app.apply_report(
        Err(ApiError {
            kind: ApiErrorKind::Server,
            message: "server error ".repeat(4),
        }),
        render_time(),
    );

    let text = render::to_text_at(&app, 80, 24, render_time());
    let header = text.lines().next().unwrap();
    let title_stem = &"AgentsView"[..5];

    assert_eq!(header.match_indices(title_stem).count(), 1, "{header}");
    assert!(header.contains("Stale"));
    assert!(header.contains("r retry"), "{header}");
    assert_width(&text, 80);
}

#[test]
fn negative_server_counts_keep_the_sign_outside_digit_grouping() {
    // If grouping treats a minus sign as a digit, an accepted negative server value puts
    // separators at positions counted from the sign instead of the magnitude.
    let mut report = activity::report();
    report.by_session[0].agent_minutes = Some(-1234.0);
    let app = app_with_report(report, ColorMode::Monochrome);

    let text = render::to_text_at(&app, 120, 40, render_time());

    assert!(text.contains("-1,234"), "{text}");
}

#[test]
fn help_overlay_clears_the_dashboard_and_lists_terminal_controls() {
    // If the overlay stops clearing or drifts outside the frame, underlying bars obscure
    // key help precisely when an operator needs recovery instructions.
    let mut report = activity::report();
    report.by_session[0].title = "Run task".to_owned();
    let mut app = app_with_report(report, ColorMode::Monochrome);
    app.handle_input(InputKey::Char('?'), activity::selection().date);

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert_golden("golden/help-80x24.txt", &text);
    for key in [
        "NAVIGATION",
        "BREAKDOWNS",
        "COMPACT VIEW",
        "GENERAL",
        "Shift-Tab",
        "cost ↔ time",
        "close dashboard",
    ] {
        assert!(text.contains(key), "missing {key:?}\n{text}");
    }
    for discarded in ["region", "close pane", "Agent-min / Cost"] {
        assert!(
            !text.contains(discarded),
            "unexpected {discarded:?}\n{text}"
        );
    }
    assert_width(&text, 80);
}

#[test]
fn long_filter_popup_scrolls_the_selected_choice_into_a_cleared_viewport() {
    // If popup viewport arithmetic regresses, a selected value near the end can be hidden
    // or drawn over uncleared dashboard cells.
    let mut report = activity::report();
    report.by_session[0].title = "Dashboard work".to_owned();
    report.partial = false;
    let mut app = app_with_report(report, ColorMode::Monochrome);
    let projects = (1..=30)
        .map(|index| ProjectInfo {
            name: format!("project-{index:02}"),
            session_count: index,
        })
        .collect();
    app.apply_projects(Ok(projects));
    app.set_project(Some("project-30".to_owned()));
    app.set_focus(Focus::Project);
    app.handle_input(InputKey::Enter, activity::selection().date);

    let text = render::to_text_at(&app, 80, 24, render_time());

    assert_golden("golden/project-popup-80x24.txt", &text);
    assert!(text.contains("  project-30"), "{text}");
    assert!(text.contains("┌ Project "));
    let filter_x = text
        .lines()
        .nth(1)
        .and_then(|line| line.split_once("Proj "))
        .map(|(before, _)| UnicodeWidthStr::width(before))
        .expect("compact filter row has a Project selector");
    let popup_x = text
        .lines()
        .nth(2)
        .and_then(|line| line.split_once("┌ Project "))
        .map(|(before, _)| UnicodeWidthStr::width(before))
        .expect("Project popup opens below the filter row");
    assert_eq!(popup_x, filter_x);
    assert_width(&text, 80);
}

#[test]
fn layout_and_capability_boundaries_are_deterministic() {
    // If breakpoints or capability mapping depend on ambient terminal state, the same pane
    // can choose a different topology or lose semantic colors between redraws.
    assert_eq!(LayoutClass::for_size(200, 50), LayoutClass::Wide);
    assert_eq!(LayoutClass::for_size(120, 40), LayoutClass::Medium);
    assert_eq!(LayoutClass::for_size(80, 24), LayoutClass::Compact);
    assert_eq!(LayoutClass::for_size(79, 24), LayoutClass::TooSmall);
    assert_eq!(
        TerminalCapabilities {
            color_count: 256,
            no_color: false,
            term_is_dumb: false,
        }
        .color_mode(),
        ColorMode::Color
    );
    for capabilities in [
        TerminalCapabilities {
            color_count: 2,
            no_color: false,
            term_is_dumb: false,
        },
        TerminalCapabilities {
            color_count: 256,
            no_color: true,
            term_is_dumb: false,
        },
        TerminalCapabilities {
            color_count: 256,
            no_color: false,
            term_is_dumb: true,
        },
    ] {
        assert_eq!(capabilities.color_mode(), ColorMode::Monochrome);
    }

    let plan = render::FramePlan::new(&ready_app(ColorMode::Color), Rect::new(0, 0, 80, 24));
    assert_eq!(plan.class(), LayoutClass::Compact);
    assert!(plan.sessions().is_some());
    assert!(plan.breakdowns().is_none());
}

#[test]
fn money_formatting_rounds_half_cents_without_overflow() {
    // If microdollars are rounded through floats or signed abs, cent boundaries drift and
    // i64::MIN can overflow in a rendering path.
    for (microdollars, expected) in [
        (0, "$0.00"),
        (1, "<$0.01"),
        (4_999, "<$0.01"),
        (5_000, "$0.01"),
        (14_999, "$0.01"),
        (15_000, "$0.02"),
        (-1, "-<$0.01"),
        (-4_999, "-<$0.01"),
        (-5_000, "-$0.01"),
        (-15_000, "-$0.02"),
    ] {
        assert_eq!(
            render::format_usd(Money { microdollars }),
            expected,
            "{microdollars}"
        );
    }
    assert!(render::format_usd(Money {
        microdollars: i64::MIN
    })
    .starts_with("-$"));
}

#[test]
fn too_small_terminal_names_the_minimum_without_panicking() {
    // If the renderer attempts the compact topology below its hard floor, rectangle math can
    // panic or silently overlap controls.
    let app = ready_app(ColorMode::Monochrome);
    let text = render::to_text_at(&app, 60, 15, render_time());

    assert!(text.contains("Need 80x24"));
    assert!(text.contains("q close"));
    assert_width(&text, 60);
}

fn assert_width(text: &str, width: usize) {
    assert!(
        text.lines()
            .all(|line| UnicodeWidthStr::width(line) <= width),
        "render exceeded {width} cells\n{text}"
    );
}

fn assert_before(text: &str, first: &str, second: &str) {
    let first = text
        .find(first)
        .unwrap_or_else(|| panic!("missing {first:?}"));
    let second = text
        .find(second)
        .unwrap_or_else(|| panic!("missing {second:?}"));
    assert!(first < second, "{text}");
}
