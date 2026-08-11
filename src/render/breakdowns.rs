// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::app::{App, BreakdownCategory, BreakdownValue, ColorMode, Focus};
use crate::wire::KeyMinutes;

use super::layout::LayoutClass;
use super::status;
use super::style::{clip_with_ellipsis, pad_right, Palette};
use super::summary::{format_count, format_usd};

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    let mode = match app.breakdown_value() {
        BreakdownValue::AgentMinutes => "Agent-min",
        BreakdownValue::Cost => "Cost",
    };
    let title = if class == LayoutClass::Compact {
        format!(
            " Breakdown · {} · {mode} ",
            category_name(app.breakdown_category())
        )
    } else {
        format!(" Breakdowns · Mode: {mode} ")
    };
    let block = palette.block(title, app.focus() == Focus::Breakdowns);
    let inner = block.inner(area);
    block.render(area, buffer);
    if status::render_report_notice(buffer, inner, app, palette) {
        return;
    }
    let Some(report) = app.report() else {
        return;
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    if report.by_project.is_empty() && report.by_model.is_empty() && report.by_agent.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            "No breakdown data for this selection",
            palette.muted(),
        )))
        .render(inner, buffer);
        return;
    }
    let categories = if class == LayoutClass::Compact {
        vec![app.breakdown_category()]
    } else {
        vec![
            BreakdownCategory::Project,
            BreakdownCategory::Model,
            BreakdownCategory::Agent,
        ]
    };
    let column_width = inner.width / categories.len() as u16;
    for (index, category) in categories.iter().enumerate() {
        let x = inner.x + index as u16 * column_width;
        let width = if index + 1 == categories.len() {
            inner.x + inner.width - x
        } else {
            column_width
        };
        let column = Rect::new(x, inner.y, width, inner.height);
        render_category(buffer, column, app, *category, index > 0, palette);
        if index > 0 {
            for y in inner.y..inner.y + inner.height {
                buffer[(x, y)].set_symbol("│").set_style(palette.muted());
            }
        }
    }
}

fn render_category(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    category: BreakdownCategory,
    has_divider: bool,
    palette: Palette,
) {
    let selected = app.breakdown_category() == category;
    let marked = selected && app.focus() == Focus::Breakdowns;
    let marker = if marked { ">" } else { " " };
    let heading = format!("{marker}{}", category_name(category));
    let heading_style = if selected {
        palette.focus()
    } else {
        palette.muted()
    };
    let heading_inset = u16::from(has_divider && marked);
    Paragraph::new(Line::from(Span::styled(
        clip_with_ellipsis(&heading, usize::from(area.width)),
        heading_style,
    )))
    .render(
        Rect::new(
            area.x + heading_inset,
            area.y,
            area.width.saturating_sub(heading_inset),
            1,
        ),
        buffer,
    );

    let category_rows = rows(app, category);
    let maximum = category_rows
        .iter()
        .map(|row| metric(row, app.breakdown_value()).0)
        .fold(0.0_f64, f64::max);
    let visible_count = usize::from(area.height.saturating_sub(1)).min(category_rows.len());
    let visible_rows = &category_rows[..visible_count];
    let value_width = breakdown_value_width(visible_rows, app.breakdown_value());
    for (index, row) in visible_rows.iter().enumerate() {
        let y = area.y + 1 + index as u16;
        let line = breakdown_line(
            row,
            app.breakdown_value(),
            maximum,
            usize::from(area.width).saturating_sub(1),
            value_width,
            palette,
        );
        Paragraph::new(line).render(Rect::new(area.x + 1, y, area.width - 1, 1), buffer);
    }
}

fn rows(app: &App, category: BreakdownCategory) -> &[KeyMinutes] {
    let Some(report) = app.report() else {
        return &[];
    };
    match category {
        BreakdownCategory::Project => &report.by_project,
        BreakdownCategory::Model => &report.by_model,
        BreakdownCategory::Agent => &report.by_agent,
    }
}

fn breakdown_line(
    row: &KeyMinutes,
    value: BreakdownValue,
    maximum: f64,
    width: usize,
    value_width: usize,
    palette: Palette,
) -> Line<'static> {
    let (total, interactive, automated) = metric(row, value);
    let label = breakdown_value_label(row, value);
    let key_width = (width / 3).clamp(8, 18);
    let bar_width = width
        .saturating_sub(key_width)
        .saturating_sub(value_width)
        .saturating_sub(2);
    let total_cells = if total <= 0.0 || maximum <= 0.0 {
        0
    } else {
        ((total / maximum) * bar_width as f64).round().max(1.0) as usize
    }
    .min(bar_width);
    let split_total = interactive + automated;
    let automated_cells = if total_cells == 0 || split_total <= 0.0 {
        0
    } else {
        ((automated / split_total) * total_cells as f64).round() as usize
    }
    .min(total_cells);
    let interactive_cells = total_cells - automated_cells;
    let automated_symbol = if palette.mode() == ColorMode::Monochrome {
        "▓"
    } else {
        "█"
    };
    Line::from(vec![
        Span::raw(pad_right(&row.key, key_width)),
        Span::raw(" "),
        Span::styled("█".repeat(interactive_cells), palette.interactive()),
        Span::styled(
            automated_symbol.repeat(automated_cells),
            palette.automated(),
        ),
        Span::raw(" "),
        Span::raw(pad_right(&label, value_width)),
    ])
}

fn breakdown_value_width(rows: &[KeyMinutes], value: BreakdownValue) -> usize {
    rows.iter()
        .map(|row| breakdown_value_label(row, value).len())
        .max()
        .unwrap_or(0)
        .max(6)
}

fn breakdown_value_label(row: &KeyMinutes, value: BreakdownValue) -> String {
    match value {
        BreakdownValue::AgentMinutes => format_breakdown_minutes(row.agent_minutes),
        BreakdownValue::Cost => format_usd(row.cost),
    }
}

fn format_breakdown_minutes(value: f64) -> String {
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

fn metric(row: &KeyMinutes, value: BreakdownValue) -> (f64, f64, f64) {
    match value {
        BreakdownValue::AgentMinutes => (
            row.agent_minutes,
            row.interactive_agent_minutes,
            row.automated_agent_minutes,
        ),
        BreakdownValue::Cost => (
            row.cost.microdollars as f64,
            row.interactive_cost.microdollars as f64,
            row.automated_cost.microdollars as f64,
        ),
    }
}

fn category_name(category: BreakdownCategory) -> &'static str {
    match category {
        BreakdownCategory::Project => "Project",
        BreakdownCategory::Model => "Model",
        BreakdownCategory::Agent => "Agent",
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use super::{breakdown_line, breakdown_value_width};
    use crate::app::{BreakdownValue, ColorMode};
    use crate::render::style::Palette;
    use crate::wire::{KeyMinutes, Money};

    #[test]
    fn lower_cost_never_draws_a_longer_bar_across_a_label_width_boundary() {
        // If each row budgets its own value label, crossing a thousands separator can make a
        // smaller cost draw a longer bar than the category maximum.
        let rows = [
            cost_row("maximum", 1_000_000_000),
            cost_row("lower", 950_000_000),
        ];
        let palette = Palette::new(ColorMode::Color);
        let value_width = breakdown_value_width(&rows, BreakdownValue::Cost);

        let maximum_line = breakdown_line(
            &rows[0],
            BreakdownValue::Cost,
            1_000_000_000.0,
            40,
            value_width,
            palette,
        );
        let lower_line = breakdown_line(
            &rows[1],
            BreakdownValue::Cost,
            1_000_000_000.0,
            40,
            value_width,
            palette,
        );

        assert!(bar_cells(&lower_line) < bar_cells(&maximum_line));
    }

    #[test]
    fn minute_breakdowns_compact_only_values_at_least_one_thousand() {
        // If large breakdown values stay fully expanded, labels consume the bar area and
        // comparisons become harder; if small values are compacted, useful precision is lost.
        for (minutes, expected) in [
            (999.0, "999"),
            (999.5, "1.0k"),
            (1_652.0, "1.7k"),
            (2_478.0, "2.5k"),
            (1_000_000.0, "1.0M"),
        ] {
            let row = KeyMinutes {
                project_key: None,
                key: "project".to_owned(),
                agent_minutes: minutes,
                cost: Money { microdollars: 0 },
                automated_agent_minutes: 0.0,
                interactive_agent_minutes: minutes,
                automated_cost: Money { microdollars: 0 },
                interactive_cost: Money { microdollars: 0 },
            };

            let line = breakdown_line(
                &row,
                BreakdownValue::AgentMinutes,
                minutes,
                40,
                6,
                Palette::new(ColorMode::Color),
            );
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();

            assert!(text.contains(expected), "missing {expected:?} in {text:?}");
        }
    }

    fn cost_row(key: &str, microdollars: i64) -> KeyMinutes {
        let cost = Money { microdollars };
        KeyMinutes {
            project_key: None,
            key: key.to_owned(),
            agent_minutes: 0.0,
            cost,
            automated_agent_minutes: 0.0,
            interactive_agent_minutes: 0.0,
            automated_cost: Money { microdollars: 0 },
            interactive_cost: cost,
        }
    }

    fn bar_cells(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .flat_map(|span| span.content.chars())
            .filter(|character| matches!(character, '█' | '▓'))
            .count()
    }
}
