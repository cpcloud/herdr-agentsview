// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, Loadable, ReportState};
use crate::wire::Automation;

use super::layout::LayoutClass;
use super::status;
use super::style::{clip_with_ellipsis, fit_with_right, Palette};

pub(super) fn render_header(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    now: DateTime<Utc>,
    palette: Palette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = usize::from(area.width);
    let status = status::header_status(app, now);
    let spinner = status::header_spinner(app, now);
    let title = "AgentsView";
    let left_width = UnicodeWidthStr::width(title) + spinner.map_or(0, |_| 2);
    let status = clip_status(&status, width.saturating_sub(left_width));
    let gap = width
        .saturating_sub(left_width)
        .saturating_sub(UnicodeWidthStr::width(status.as_str()));
    let status_style = match app.report_state() {
        ReportState::Stale { .. } => palette.warning(),
        ReportState::Failed(_) => palette.error(),
        ReportState::InitialLoading
        | ReportState::Ready { .. }
        | ReportState::Refreshing { .. } => palette.muted(),
    };
    let mut line = vec![Span::styled(title, palette.focus())];
    if let Some(spinner) = spinner {
        line.push(Span::raw(" "));
        line.push(Span::styled(spinner, palette.warning()));
    }
    line.push(Span::raw(" ".repeat(gap)));
    line.push(Span::styled(status, status_style));
    Paragraph::new(Line::from(line)).render(Rect::new(area.x, area.y, area.width, 1), buffer);

    if area.height < 2 {
        return;
    }
    let spans = filter_spans(app, class, width, palette);
    Paragraph::new(Line::from(spans)).render(Rect::new(area.x, area.y + 1, area.width, 1), buffer);
}

fn clip_status(status: &str, width: usize) -> String {
    if UnicodeWidthStr::width(status) <= width {
        return status.to_owned();
    }
    status.strip_suffix(status::RECOVERY_HINT).map_or_else(
        || clip_with_ellipsis(status, width),
        |prefix| fit_with_right(prefix, status::RECOVERY_HINT, width),
    )
}

pub(super) fn render_compact_rail(buffer: &mut Buffer, area: Rect, app: &App, palette: Palette) {
    let sessions = if app.compact_region() == crate::app::CompactRegion::Sessions {
        palette.focus()
    } else {
        Style::default()
    };
    let breakdown = if app.compact_region() == crate::app::CompactRegion::Breakdown {
        palette.focus()
    } else {
        Style::default()
    };
    let line = Line::from(vec![
        Span::styled("Sections: ", palette.muted()),
        Span::styled(" s ", palette.keycap()),
        Span::styled(" Sessions", sessions),
        Span::raw("  "),
        Span::styled(" b ", palette.keycap()),
        Span::styled(" Breakdown", breakdown),
        Span::raw("  "),
        Span::styled(" p/m/a ", palette.keycap()),
        Span::raw(" category"),
    ]);
    Paragraph::new(line).render(area, buffer);
}

pub(super) fn render_footer(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    let hints = app.contextual_keys();
    let use_compact =
        class == LayoutClass::Compact || footer_width(&hints, false, 2) > usize::from(area.width);
    let gap = if use_compact { 1 } else { 2 };
    let mut spans = Vec::new();
    for (index, hint) in hints.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        let (key, action) = if use_compact {
            hint.compact.split_once(' ').unwrap_or((hint.compact, ""))
        } else {
            (hint.key, hint.action)
        };
        spans.push(Span::styled(format!(" {key} "), palette.keycap()));
        if !action.is_empty() {
            spans.push(Span::styled(format!(" {action}"), palette.muted()));
        }
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn footer_width(hints: &[crate::app::KeyHint], compact: bool, gap: usize) -> usize {
    let hints_width = hints
        .iter()
        .map(|hint| {
            let (key, action) = if compact {
                hint.compact.split_once(' ').unwrap_or((hint.compact, ""))
            } else {
                (hint.key, hint.action)
            };
            UnicodeWidthStr::width(key)
                + 2
                + if action.is_empty() {
                    0
                } else {
                    UnicodeWidthStr::width(action) + 1
                }
        })
        .sum::<usize>();
    hints_width + hints.len().saturating_sub(1) * gap
}

fn filter_spans(
    app: &App,
    class: LayoutClass,
    width: usize,
    palette: Palette,
) -> Vec<Span<'static>> {
    let tokens = filter_tokens(app, class, width, palette);
    let mut spans = Vec::with_capacity(tokens.len() * 2 - 1);
    for (index, token) in tokens.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", palette.muted()));
        }
        spans.push(token.span);
    }
    spans
}

struct FilterView {
    focus: Focus,
    full_label: &'static str,
    compact_label: &'static str,
    value: String,
    failed: bool,
}

impl FilterView {
    fn label(&self, class: LayoutClass) -> &'static str {
        if class == LayoutClass::Compact {
            self.compact_label
        } else {
            self.full_label
        }
    }
}

struct FilterToken {
    focus: Focus,
    span: Span<'static>,
}

fn filter_tokens(
    app: &App,
    class: LayoutClass,
    width: usize,
    palette: Palette,
) -> Vec<FilterToken> {
    let views = filter_views(app, class == LayoutClass::Compact);
    let labels_and_separators = views
        .iter()
        .map(|view| UnicodeWidthStr::width(view.label(class)) + 1)
        .sum::<usize>()
        + views.len().saturating_sub(1) * 3;
    let available = width.saturating_sub(labels_and_separators);
    let value_widths = filter_value_widths(available, &views);
    views
        .into_iter()
        .zip(value_widths)
        .map(|(view, value_width)| {
            let value = clip_with_ellipsis(&view.value, value_width);
            let focused = app.focus() == view.focus;
            let style = if view.failed && focused {
                palette.focused_warning()
            } else if view.failed {
                palette.warning()
            } else if focused {
                palette.selected()
            } else {
                Style::default()
            };
            FilterToken {
                focus: view.focus,
                span: Span::styled(format!("{} {value}", view.label(class)), style),
            }
        })
        .collect()
}

pub(super) fn filter_anchor(
    app: &App,
    class: LayoutClass,
    width: usize,
    focus: Focus,
) -> Option<(u16, u16)> {
    let tokens = filter_tokens(app, class, width, Palette::new(app.color_mode()));
    let index = tokens.iter().position(|token| token.focus == focus)?;
    let x = tokens
        .iter()
        .take(index)
        .map(|token| UnicodeWidthStr::width(token.span.content.as_ref()) + 3)
        .sum::<usize>();
    let token_width = UnicodeWidthStr::width(tokens.get(index)?.span.content.as_ref());
    Some((x as u16, token_width as u16))
}

fn filter_views(app: &App, compact: bool) -> [FilterView; 5] {
    [
        FilterView {
            focus: Focus::Date,
            full_label: "Date",
            compact_label: "Date",
            value: app.selection().date.to_string(),
            failed: false,
        },
        FilterView {
            focus: Focus::Project,
            full_label: "Project",
            compact_label: "Proj",
            value: metadata_value(app.selection().project.as_deref(), app.projects(), compact),
            failed: matches!(app.projects(), Loadable::Failed(_)),
        },
        FilterView {
            focus: Focus::Agent,
            full_label: "Agent",
            compact_label: "Agent",
            value: metadata_value(app.selection().agent.as_deref(), app.agents(), compact),
            failed: matches!(app.agents(), Loadable::Failed(_)),
        },
        FilterView {
            focus: Focus::Machine,
            full_label: "Machine",
            compact_label: "Host",
            value: metadata_value(app.selection().machine.as_deref(), app.machines(), compact),
            failed: matches!(app.machines(), Loadable::Failed(_)),
        },
        FilterView {
            focus: Focus::Automation,
            full_label: "Session",
            compact_label: "Session",
            value: match app.selection().automation {
                Automation::All => "All".to_owned(),
                Automation::Interactive => "Interactive".to_owned(),
                Automation::Automated => "Automated".to_owned(),
            },
            failed: false,
        },
    ]
}

fn filter_value_widths(available: usize, views: &[FilterView; 5]) -> [usize; 5] {
    let natural: [usize; 5] =
        std::array::from_fn(|index| UnicodeWidthStr::width(views[index].value.as_str()));
    let minimum = [10, 3, 3, 3, 3];
    let mut widths = std::array::from_fn(|index| natural[index].min(minimum[index]));
    let mut remaining = available.saturating_sub(widths.iter().sum());

    let automation = natural[4].saturating_sub(widths[4]).min(remaining);
    widths[4] += automation;
    remaining -= automation;

    while remaining > 0 {
        let mut allocated = false;
        for index in [1, 2, 3] {
            if widths[index] < natural[index] {
                widths[index] += 1;
                remaining -= 1;
                allocated = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !allocated {
            break;
        }
    }
    widths
}

fn metadata_value<T>(selected: Option<&str>, state: &Loadable<Vec<T>>, compact: bool) -> String {
    match state {
        Loadable::Failed(_) if compact => "!".to_owned(),
        Loadable::Failed(_) => "unavailable".to_owned(),
        Loadable::Loading if compact => selected.unwrap_or("…").to_owned(),
        Loadable::Loading => selected.unwrap_or("loading…").to_owned(),
        Loadable::Ready(_) => selected.unwrap_or("All").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::NaiveDate;
    use chrono_tz::UTC;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

    use super::{filter_spans, render_footer, render_header};
    use crate::api::{ApiError, ApiErrorKind};
    use crate::app::{App, Focus};
    use crate::render::layout::LayoutClass;
    use crate::render::style::Palette;
    use crate::wire::ReportSelection;

    fn app() -> App {
        App::new(
            ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 9).unwrap(), UTC),
            Duration::from_secs(300),
        )
    }

    #[test]
    fn focused_filter_uses_background_instead_of_a_chevron() {
        // If filter focus falls back to a text marker, the row shifts as focus moves and the
        // selector reads like command-line syntax instead of a stable navigation strip.
        let mut app = app();
        app.set_focus(Focus::Automation);
        let spans = filter_spans(
            &app,
            LayoutClass::Medium,
            120,
            Palette::new(crate::app::ColorMode::Color),
        );
        let focused = spans.last().expect("Session filter is last");

        assert_eq!(focused.content, "Session All");
        assert_eq!(focused.style.bg, Some(Color::LightBlue));

        let monochrome = filter_spans(
            &app,
            LayoutClass::Medium,
            120,
            Palette::new(crate::app::ColorMode::Monochrome),
        );
        assert!(monochrome
            .last()
            .expect("Session filter is last")
            .style
            .add_modifier
            .contains(Modifier::REVERSED));
    }

    #[test]
    fn moving_filter_focus_does_not_move_filter_text() {
        // If focus consumes a character or changes a selector's width, every Tab press makes
        // the remaining filters jump horizontally even though their values did not change.
        let mut app = app();
        let text_for = |app: &App| {
            filter_spans(
                app,
                LayoutClass::Medium,
                120,
                Palette::new(crate::app::ColorMode::Color),
            )
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
        };
        let date_focused = text_for(&app);
        app.set_focus(Focus::Project);
        let project_focused = text_for(&app);
        app.set_focus(Focus::Automation);
        let session_focused = text_for(&app);

        assert_eq!(date_focused, project_focused);
        assert_eq!(date_focused, session_focused);
    }

    #[test]
    fn failed_focused_filter_keeps_a_background_focus_cue() {
        // If warning styling replaces selected styling, a failed selector has no visible
        // response to Tab and the retry target becomes ambiguous.
        let mut app = app();
        app.apply_projects(Err(ApiError {
            kind: ApiErrorKind::Network,
            message: "offline".to_owned(),
        }));
        app.set_focus(Focus::Project);
        let spans = filter_spans(
            &app,
            LayoutClass::Medium,
            120,
            Palette::new(crate::app::ColorMode::Color),
        );

        assert_eq!(spans[2].content, "Project unavailable");
        assert!(spans[2].style.bg.is_some());
    }

    #[test]
    fn footer_renders_shortcuts_as_white_keycaps() {
        // If footer hints are flattened into one muted string, keys cannot be picked out from
        // their actions at a glance in a keyboard-first interface.
        let app = app();
        let area = Rect::new(0, 0, 120, 1);
        let mut buffer = Buffer::empty(area);
        render_footer(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(crate::app::ColorMode::Color),
        );

        for x in 0..=4 {
            assert_eq!(buffer[(x, 0)].bg, Color::White, "cell {x}");
        }
        assert_eq!(buffer[(5, 0)].bg, Color::Reset);
    }

    #[test]
    fn footer_keeps_the_complete_quit_hint_at_supported_widths() {
        // If styled padding is not included in width selection, the final global shortcut is
        // clipped even though compact hint copy fits the supported terminal.
        let mut app = app();
        app.begin_foreground_load();
        app.set_focus(Focus::Sessions);
        for (class, width, expected) in [
            (LayoutClass::Compact, 80, "q  quit"),
            (LayoutClass::Medium, 120, "q  close dashboard"),
            (LayoutClass::Wide, 200, "q  close dashboard"),
        ] {
            let area = Rect::new(0, 0, width, 1);
            let mut buffer = Buffer::empty(area);
            render_footer(
                &mut buffer,
                area,
                &app,
                class,
                Palette::new(crate::app::ColorMode::Color),
            );
            let line = (0..width)
                .map(|x| buffer[(x, 0)].symbol())
                .collect::<String>();
            assert!(line.contains(expected), "missing {expected:?}\n{line}");
        }
    }

    #[test]
    fn page_title_starts_at_the_dashboard_edge() {
        // If the title keeps decorative left padding, the only unboxed heading looks
        // accidentally indented relative to every full-width panel below it.
        let app = app();
        let area = Rect::new(0, 0, 120, 2);
        let mut buffer = Buffer::empty(area);

        render_header(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            "2026-08-08T17:21:12Z".parse().unwrap(),
            Palette::new(crate::app::ColorMode::Color),
        );

        assert_eq!(buffer[(0, 0)].symbol(), "A");
    }

    #[test]
    fn loading_spinner_sits_beside_the_agentsview_title_in_an_accent_color() {
        // If loading stays right-aligned or shares the muted status style, it is easy to
        // miss while a slow Activity query is still running.
        let app = app();
        let area = Rect::new(0, 0, 120, 2);
        let mut buffer = Buffer::empty(area);

        render_header(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            "1970-01-01T00:00:00Z".parse().unwrap(),
            Palette::new(crate::app::ColorMode::Color),
        );

        let title = (0..12).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(title, "AgentsView ⠖");
        assert_eq!(buffer[(11, 0)].fg, Color::Yellow);
    }
}
