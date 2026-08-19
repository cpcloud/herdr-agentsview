// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;
use crate::wire::Money;

use super::layout::LayoutClass;
use super::style::{clip_with_ellipsis, Palette};
use super::time::format_clock;

struct Metric {
    label: &'static str,
    value: Vec<Span<'static>>,
}

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    let title = app.report().filter(|report| report.partial).map_or_else(
        || " Summary ".to_owned(),
        |report| {
            format!(
                " Summary · through {} ",
                format_clock(report.effective_end, report.timezone)
            )
        },
    );
    let block = palette.block(title, false);
    let inner = block.inner(area);
    block.render(area, buffer);
    let Some(report) = app.report() else {
        return;
    };

    let peak_time = report
        .peak
        .at
        .map_or_else(|| "—".to_owned(), |at| format_clock(at, report.timezone));
    let approximate_agent_minutes = report.totals.agent_minutes.round().abs() >= 1_000.0;
    let agent_minutes = if approximate_agent_minutes {
        format_compact_count(report.totals.agent_minutes)
    } else {
        format_count(report.totals.agent_minutes.round() as i128)
    };
    let number = palette.summary_number();
    let interactive = palette.summary_interactive();
    let automated = palette.summary_automated();
    let separator = palette.muted();
    let metrics = [
        Metric {
            label: "Agents",
            value: vec![
                Span::styled("▲ ", number),
                Span::styled(format_count(report.peak.agents as i128), number),
                Span::styled(" @ ", separator),
                Span::styled(peak_time, number),
            ],
        },
        Metric {
            label: "Time",
            value: vec![
                Span::styled("● ", interactive),
                Span::styled(format_minutes(report.totals.active_minutes), interactive),
                Span::raw("  "),
                Span::styled("○ ", number),
                Span::styled(format_minutes(report.totals.idle_minutes), number),
            ],
        },
        Metric {
            label: "Work",
            value: vec![
                Span::styled("◷ ", number),
                Span::styled(if approximate_agent_minutes { "~" } else { "" }, separator),
                Span::styled(agent_minutes, number),
                Span::styled(" / ", separator),
                Span::styled(format_usd(report.totals.cost), number),
            ],
        },
        Metric {
            label: "Sessions",
            value: vec![
                Span::styled(format_count(report.totals.sessions as i128), number),
                Span::styled(" = ", separator),
                Span::styled(
                    format!(
                        "{} int",
                        format_count(report.totals.interactive_sessions as i128)
                    ),
                    interactive,
                ),
                Span::styled(" + ", separator),
                Span::styled(
                    format!(
                        "{} auto",
                        format_count(report.totals.automated_sessions as i128)
                    ),
                    automated,
                ),
                Span::styled(" · ", separator),
                Span::styled(format_count(report.totals.untimed_sessions as i128), number),
                Span::raw(" unt"),
            ],
        },
        Metric {
            label: "Projects",
            value: vec![
                Span::styled(
                    format_count(report.totals.distinct_projects as i128),
                    number,
                ),
                Span::styled(" · ", separator),
                Span::styled(format_count(report.totals.distinct_models as i128), number),
                Span::raw(" models"),
            ],
        },
    ];

    match class {
        LayoutClass::Compact => render_compact(buffer, inner, metrics, palette),
        LayoutClass::Wide | LayoutClass::Medium => render_columns(buffer, inner, metrics, palette),
        LayoutClass::TooSmall => {}
    }
}

fn render_compact(buffer: &mut Buffer, area: Rect, metrics: [Metric; 5], palette: Palette) {
    let [agents, time, work, sessions, projects] = metrics;
    render_compact_pair(
        buffer,
        Rect::new(area.x, area.y, area.width, 1),
        agents,
        time,
        palette,
    );
    render_compact_pair(
        buffer,
        Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
        work,
        projects,
        palette,
    );
    render_inline_metric(
        buffer,
        Rect::new(area.x, area.y.saturating_add(2), area.width, 1),
        sessions,
    );
}

fn render_compact_pair(
    buffer: &mut Buffer,
    area: Rect,
    left: Metric,
    right: Metric,
    palette: Palette,
) {
    let content_width = area.width.saturating_sub(3);
    let left_width = content_width / 2;
    let divider_x = area.x.saturating_add(left_width).saturating_add(1);
    render_inline_metric(
        buffer,
        Rect::new(area.x, area.y, left_width, area.height),
        left,
    );
    if divider_x < area.right() {
        buffer[(divider_x, area.y)]
            .set_symbol("│")
            .set_style(palette.muted());
    }
    let right_x = divider_x.saturating_add(2);
    render_inline_metric(
        buffer,
        Rect::new(
            right_x,
            area.y,
            area.right().saturating_sub(right_x),
            area.height,
        ),
        right,
    );
}

fn render_inline_metric(buffer: &mut Buffer, area: Rect, metric: Metric) {
    let mut spans = vec![Span::raw(format!("{}  ", metric.label))];
    spans.extend(metric.value);
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn render_columns(buffer: &mut Buffer, area: Rect, metrics: [Metric; 5], palette: Palette) {
    const WEIGHTS: [u16; 5] = [20, 16, 18, 34, 12];
    let available = area.width.saturating_sub((metrics.len() - 1) as u16);
    let mut x = area.x;
    let mut allocated = 0;
    for (index, (metric, weight)) in metrics.into_iter().zip(WEIGHTS).enumerate() {
        if index > 0 {
            for y in area.y..area.y + area.height {
                buffer[(x, y)].set_symbol("│").set_style(palette.muted());
            }
            x = x.saturating_add(1);
        }
        let width = if index + 1 == WEIGHTS.len() {
            available.saturating_sub(allocated)
        } else {
            available.saturating_mul(weight) / 100
        };
        allocated = allocated.saturating_add(width);
        let inset = u16::from(index > 0 && width > 0);
        let cell = Rect::new(
            x.saturating_add(inset),
            area.y,
            width.saturating_sub(inset),
            2,
        );
        render_metric_group(buffer, cell, metric);
        x = x.saturating_add(width);
    }
}

fn render_metric_group(buffer: &mut Buffer, area: Rect, metric: Metric) {
    let width = usize::from(area.width);
    let lines = vec![
        Line::from(clip_with_ellipsis(metric.label, width)),
        Line::from(metric.value),
    ];
    Paragraph::new(lines).render(area, buffer);
}

pub fn format_usd(money: Money) -> String {
    let value = i128::from(money.microdollars);
    if value == 0 {
        return "$0.00".to_owned();
    }
    let negative = value < 0;
    let magnitude = value.abs();
    if magnitude < 5_000 {
        return if negative {
            "-<$0.01".to_owned()
        } else {
            "<$0.01".to_owned()
        };
    }
    let cents = (magnitude + 5_000) / 10_000;
    let dollars = cents / 100;
    let cents = cents % 100;
    let sign = if negative { "-" } else { "" };
    format!("{sign}${}.{:02}", format_count(dollars), cents)
}

pub(super) fn format_minutes(minutes: f64) -> String {
    let rounded = minutes.round();
    if rounded >= 60.0 {
        let total = rounded as i128;
        let hours = total / 60;
        let minutes = total % 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{minutes:02}")
        }
    } else if (minutes - rounded).abs() < f64::EPSILON {
        format!("{}m", rounded as i128)
    } else {
        format!("{minutes:.1}m")
    }
}

pub(super) fn format_compact_count(value: f64) -> String {
    if value.round().abs() < 1_000.0 {
        return format_count(value.round() as i128);
    }

    const SUFFIXES: [&str; 6] = ["", "k", "M", "B", "T", "P"];
    let mut scaled = value;
    let mut suffix = 0;
    while scaled.round().abs() >= 1_000.0 && suffix + 1 < SUFFIXES.len() {
        scaled /= 1_000.0;
        suffix += 1;
    }

    let mut rounded = (scaled * 10.0).round() / 10.0;
    if rounded.abs() >= 1_000.0 && suffix + 1 < SUFFIXES.len() {
        rounded /= 1_000.0;
        suffix += 1;
    }
    format!("{rounded:.1}{}", SUFFIXES[suffix])
}

pub(super) fn format_count(value: i128) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let mut grouped =
        String::with_capacity(digits.len() + digits.len() / 3 + usize::from(negative));
    if negative {
        grouped.push('-');
    }
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

    use super::render;
    use crate::app::{App, ColorMode};
    use crate::render::layout::LayoutClass;
    use crate::render::style::Palette;
    use crate::wire::{Money, Report, ReportSelection};

    #[test]
    fn medium_summary_uses_compact_metric_vocabulary() {
        // If browser-length labels or repeated units return, the summary consumes terminal
        // width without improving the operator's scan path.
        let app = fixture_app(ColorMode::Color);
        let area = Rect::new(0, 0, 120, 4);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let headers = buffer_line(&buffer, 1);
        let values = buffer_line(&buffer, 2);
        for header in ["Agents", "Time", "Work", "Sessions", "Projects"] {
            assert!(headers.contains(header), "missing {header:?}\n{headers}");
        }
        assert!(!headers.contains("Concurrent"), "{headers}");
        for metric in [
            "▲ 13 @ 07:34",
            "● 2h04  ○ 6h33",
            "◷ ~1.2k / $355.11",
            "69 = 60 int + 9 auto · 38 unt",
            "7 · 3 models",
        ] {
            assert!(values.contains(metric), "missing {metric:?}\n{values}");
        }
    }

    #[test]
    fn color_summary_numbers_use_semantic_roles() {
        // If summary values fall back to the default foreground, symbols and compact copy
        // alone cannot create the number-led scan path used by the Activity UI.
        let app = fixture_app(ColorMode::Color);
        let area = Rect::new(0, 0, 120, 4);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        for (needle, color) in [
            ("13", Color::LightCyan),
            ("07:34", Color::LightCyan),
            ("2h04", Color::LightBlue),
            ("6h33", Color::LightCyan),
            ("1.2k", Color::LightCyan),
            ("$355.11", Color::LightCyan),
            ("69 =", Color::LightCyan),
            ("60 int", Color::LightBlue),
            ("9 auto", Color::LightYellow),
            ("38 unt", Color::LightCyan),
            ("7 ·", Color::LightCyan),
            ("3 models", Color::LightCyan),
        ] {
            let cell = find_cell(&buffer, 2, needle);
            assert_eq!(cell.fg, color, "wrong color for {needle:?}");
            assert!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                "{needle:?} is not bold",
            );
        }
    }

    #[test]
    fn monochrome_summary_numbers_keep_their_weight() {
        // If color fallback removes emphasis or dims automated counts, color-limited
        // terminals lose the numeric scan path instead of degrading by color alone.
        let app = fixture_app(ColorMode::Monochrome);
        let area = Rect::new(0, 0, 120, 4);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Monochrome),
        );

        for needle in [
            "13", "07:34", "2h04", "6h33", "1.2k", "$355.11", "69 =", "60 int", "9 auto", "38 unt",
            "7 ·", "3 models",
        ] {
            let cell = find_cell(&buffer, 2, needle);
            assert_eq!(cell.fg, Color::Reset, "unexpected color for {needle:?}");
            assert!(
                cell.style().add_modifier.contains(Modifier::BOLD),
                "{needle:?} is not bold",
            );
        }
    }

    fn fixture_app(color_mode: ColorMode) -> App {
        let mut report: Report =
            serde_json::from_str(include_str!("../../tests/fixtures/report-v6.json")).unwrap();
        report.peak.agents = 13;
        report.peak.at = Some("2026-08-19T07:34:00-04:00".parse().unwrap());
        report.totals.active_minutes = 124.0;
        report.totals.idle_minutes = 393.0;
        report.totals.agent_minutes = 1_154.0;
        report.totals.cost = Money {
            microdollars: 355_110_000,
        };
        report.totals.sessions = 69;
        report.totals.interactive_sessions = 60;
        report.totals.automated_sessions = 9;
        report.totals.untimed_sessions = 38;
        report.totals.distinct_projects = 7;
        report.totals.distinct_models = 3;
        let selection = ReportSelection::new(report.range_start.date_naive(), report.timezone);
        let mut app = App::new(selection, Duration::from_secs(300));
        app.set_color_mode(color_mode);
        app.begin_foreground_load();
        app.apply_report(
            Ok(Box::new(report)),
            "2026-08-19T12:00:00Z".parse().unwrap(),
        );
        app
    }

    fn buffer_line(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn find_cell<'a>(buffer: &'a Buffer, y: u16, needle: &str) -> &'a ratatui::buffer::Cell {
        let symbols = needle
            .chars()
            .map(|symbol| symbol.to_string())
            .collect::<Vec<_>>();
        let start = (0..buffer.area.width)
            .find(|start| {
                symbols.iter().enumerate().all(|(offset, symbol)| {
                    start.saturating_add(offset as u16) < buffer.area.width
                        && buffer[(*start + offset as u16, y)].symbol() == symbol
                })
            })
            .unwrap_or_else(|| panic!("missing {needle:?} in {:?}", buffer_line(buffer, y)));
        &buffer[(start, y)]
    }
}
