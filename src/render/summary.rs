use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;
use crate::wire::Money;

use super::layout::LayoutClass;
use super::style::{clip_with_ellipsis, Palette};
use super::time::format_clock;

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
    let peak = format!("Peak {} @ {peak_time}", report.peak.agents);
    let active = format!("Active {}", format_minutes(report.totals.active_minutes));
    let idle = format!("Idle {}", format_minutes(report.totals.idle_minutes));
    let agent_minutes = format!(
        "{} agent-min",
        format_count(report.totals.agent_minutes.round() as i128)
    );
    let cost = format!("{} cost", format_usd(report.totals.cost));
    let sessions = format!("{} total", format_count(report.totals.sessions as i128));
    let session_split = if class == LayoutClass::Wide {
        format!(
            "{} interactive · {} automated · {} untimed",
            format_count(report.totals.interactive_sessions as i128),
            format_count(report.totals.automated_sessions as i128),
            format_count(report.totals.untimed_sessions as i128),
        )
    } else {
        format!(
            "{} int · {} auto · {} untimed",
            format_count(report.totals.interactive_sessions as i128),
            format_count(report.totals.automated_sessions as i128),
            format_count(report.totals.untimed_sessions as i128),
        )
    };
    let projects = format!("{} projects", report.totals.distinct_projects);
    let models = format!("{} models", report.totals.distinct_models);

    match class {
        LayoutClass::Compact => render_compact(
            buffer,
            inner,
            [
                (
                    "Concurrent agents",
                    peak,
                    "Time",
                    format!("{active} · {idle}"),
                ),
                (
                    "Work",
                    format!("{agent_minutes} · {cost}"),
                    "Scope",
                    format!("{projects} · {models}"),
                ),
            ],
            sessions,
            session_split,
            palette,
        ),
        LayoutClass::Wide | LayoutClass::Medium => render_columns(
            buffer,
            inner,
            [
                ("Concurrent agents", peak, String::new()),
                ("Time", active, idle),
                ("Work", agent_minutes, cost),
                ("Sessions", sessions, session_split),
                ("Scope", projects, models),
            ],
            palette,
        ),
        LayoutClass::TooSmall => {}
    }
}

fn render_compact(
    buffer: &mut Buffer,
    area: Rect,
    pairs: [(&'static str, String, &'static str, String); 2],
    sessions: String,
    split: String,
    palette: Palette,
) {
    let mut lines = pairs
        .into_iter()
        .map(|(left_label, left_value, right_label, right_value)| {
            Line::from(vec![
                Span::styled(format!("{left_label}  "), palette.muted()),
                Span::raw(left_value),
                Span::styled("  │  ", palette.muted()),
                Span::styled(format!("{right_label}  "), palette.muted()),
                Span::raw(right_value),
            ])
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(vec![
        Span::styled("Sessions  ", palette.muted()),
        Span::raw(format!("{sessions} · {split}")),
    ]));
    Paragraph::new(lines).render(area, buffer);
}

fn render_columns(
    buffer: &mut Buffer,
    area: Rect,
    groups: [(&'static str, String, String); 5],
    palette: Palette,
) {
    const WEIGHTS: [u16; 5] = [20, 16, 18, 34, 12];
    let available = area.width.saturating_sub((groups.len() - 1) as u16);
    let mut x = area.x;
    let mut allocated = 0;
    for (index, ((label, primary, secondary), weight)) in
        groups.into_iter().zip(WEIGHTS).enumerate()
    {
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
            3,
        );
        render_metric_group(buffer, cell, label, &primary, &secondary, palette);
        x = x.saturating_add(width);
    }
}

fn render_metric_group(
    buffer: &mut Buffer,
    area: Rect,
    label: &str,
    primary: &str,
    secondary: &str,
    palette: Palette,
) {
    let width = usize::from(area.width);
    let lines = vec![
        Line::from(Span::styled(
            clip_with_ellipsis(label, width),
            palette.muted(),
        )),
        Line::from(clip_with_ellipsis(primary, width)),
        Line::from(clip_with_ellipsis(secondary, width)),
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
