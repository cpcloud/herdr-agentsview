// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeSet, HashMap};

use chrono_tz::Tz;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, SessionSortColumn, SortDirection};
use crate::wire::SessionRow;

use super::layout::LayoutClass;
use super::status;
use super::style::{clip_with_ellipsis, pad_right, Palette};
use super::summary::{format_count, format_usd};
use super::time::{format_interval, format_window};

struct ColumnSpec {
    sort: SessionSortColumn,
    label: &'static str,
    minimum: usize,
    flexible: bool,
    cell: fn(&SessionRow, Tz) -> SessionCell,
}

const fn column(
    sort: SessionSortColumn,
    label: &'static str,
    minimum: usize,
    flexible: bool,
    cell: fn(&SessionRow, Tz) -> SessionCell,
) -> ColumnSpec {
    ColumnSpec {
        sort,
        label,
        minimum,
        flexible,
        cell,
    }
}

const FULL_COLUMNS: [ColumnSpec; 7] = [
    column(
        SessionSortColumn::Session,
        "Session",
        16,
        true,
        session_cell,
    ),
    column(SessionSortColumn::Model, "Model", 13, true, model_cell),
    column(
        SessionSortColumn::Project,
        "Project",
        13,
        true,
        project_cell,
    ),
    column(SessionSortColumn::Agent, "Agent", 8, true, agent_cell),
    column(
        SessionSortColumn::AgentMinutes,
        "Min",
        7,
        false,
        agent_minutes_cell,
    ),
    column(SessionSortColumn::Cost, "Cost", 9, false, cost_cell),
    column(SessionSortColumn::Window, "Window", 11, true, window_cell),
];

const COMPACT_COLUMNS: [ColumnSpec; 4] = [
    column(
        SessionSortColumn::Session,
        "Session",
        18,
        true,
        session_cell,
    ),
    column(
        SessionSortColumn::Project,
        "Project",
        14,
        true,
        project_cell,
    ),
    column(
        SessionSortColumn::AgentMinutes,
        "Min",
        7,
        false,
        agent_minutes_cell,
    ),
    column(SessionSortColumn::Cost, "Cost", 9, false, cost_cell),
];

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    let compact = class == LayoutClass::Compact;
    let rows = app.displayed_sessions();
    let inspected_bucket = app.inspected_bucket();
    let title = match (app.report(), inspected_bucket) {
        (None, _) => " Sessions ".to_owned(),
        (Some(report), Some(bucket)) if report.bucket_is_future(bucket) => {
            let title = format!(
                " Sessions in {} (future) ",
                format_interval(bucket.start, bucket.end, report.timezone),
            );
            with_compact_sort(title, app, compact)
        }
        (Some(report), Some(bucket)) => {
            let title = format!(
                " Sessions in {} ({}) ",
                format_interval(
                    bucket.start,
                    report.observed_bucket_end(bucket),
                    report.timezone,
                ),
                rows.len()
            );
            with_compact_sort(title, app, compact)
        }
        (Some(report), None) if compact => format!(
            " Sessions ({} total) · Sort: {}{} ",
            report.totals.sessions,
            app.sort_column().name(),
            sort_arrow(app.sort_direction())
        ),
        (Some(report), None) => format!(" Sessions ({} total) ", report.totals.sessions),
    };
    let block = palette.block(title, app.focus() == Focus::Sessions);
    let inner = block.inner(area);
    block.render(area, buffer);
    if status::render_report_notice(buffer, inner, app, palette) {
        return;
    }
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let Some(report) = app.report() else {
        return;
    };
    let timezone = report.timezone;
    let category_colors = CategoryColors::new(&report.by_session);

    let columns = if compact {
        &COMPACT_COLUMNS[..]
    } else {
        &FULL_COLUMNS[..]
    };
    let widths = column_widths(usize::from(inner.width), columns);
    let headers = columns
        .iter()
        .map(|column| sort_header(app, column))
        .collect::<Vec<_>>();
    let header = compose_row(" ", &headers, &widths);
    Paragraph::new(Line::from(Span::styled(header, palette.muted())))
        .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);

    let row_geometry = session_rows(area, class);
    let detail_rows = row_geometry.detail;
    let visible = row_geometry.visible;
    let cursor = app.session_cursor().min(rows.len().saturating_sub(1));
    let scroll = app.session_scroll_for_viewport(rows.len(), visible);

    for (visible_index, row) in rows.iter().skip(scroll).take(visible).enumerate() {
        let row_index = scroll + visible_index;
        let selected = row_index == cursor;
        let marker = if selected && app.focus() == Focus::Sessions {
            ">"
        } else {
            " "
        };
        let cells = columns
            .iter()
            .map(|column| (column.cell)(row, timezone))
            .collect::<Vec<_>>();
        let row_style = if selected && app.focus() == Focus::Sessions {
            palette.session_selected()
        } else {
            Style::default()
        };
        let line = compose_styled_row(
            marker,
            &cells,
            &widths,
            &category_colors,
            row_style,
            palette,
        );
        let y = inner.y + 1 + visible_index as u16;
        if y < inner.y + inner.height {
            Paragraph::new(line).render(Rect::new(inner.x, y, inner.width, 1), buffer);
        }
    }

    if compact && detail_rows == 1 {
        if let Some(row) = rows.get(cursor) {
            let detail = compact_detail(
                row,
                timezone,
                usize::from(inner.width),
                &category_colors,
                palette,
            );
            let y = inner.y + inner.height - 1;
            Paragraph::new(detail).render(Rect::new(inner.x, y, inner.width, 1), buffer);
        }
    }
}

fn with_compact_sort(mut title: String, app: &App, compact: bool) -> String {
    if compact {
        title.truncate(title.trim_end().len());
        title.push_str(&format!(
            " · Sort: {}{} ",
            app.sort_column().name(),
            sort_arrow(app.sort_direction())
        ));
    }
    title
}

fn compact_detail(
    row: &SessionRow,
    timezone: Tz,
    width: usize,
    colors: &CategoryColors,
    palette: Palette,
) -> Line<'static> {
    let window = window(row, timezone);
    let frame = format!("Model:  · Agent:  · Window: {window}");
    let values_width = width.saturating_sub(UnicodeWidthStr::width(frame.as_str()));
    let model_width = UnicodeWidthStr::width(row.primary_model.as_str())
        .min(values_width.div_ceil(2))
        .max(values_width.saturating_sub(UnicodeWidthStr::width(row.agent.as_str())));
    let agent_width = values_width.saturating_sub(model_width);
    Line::from(vec![
        Span::styled("Model: ", palette.muted()),
        Span::styled(
            clip_with_ellipsis(&row.primary_model, model_width),
            colors.style(CategoryKind::Model, &row.primary_model, palette),
        ),
        Span::styled(" · Agent: ", palette.muted()),
        Span::styled(
            clip_with_ellipsis(&row.agent, agent_width),
            colors.style(CategoryKind::Agent, &row.agent, palette),
        ),
        Span::styled(format!(" · Window: {window}"), palette.muted()),
    ])
}

pub(super) fn viewport_rows(area: Rect, class: LayoutClass) -> usize {
    session_rows(area, class).visible
}

#[derive(Clone, Copy)]
struct SessionRows {
    detail: usize,
    visible: usize,
}

fn session_rows(area: Rect, class: LayoutClass) -> SessionRows {
    let inner_height = usize::from(area.height.saturating_sub(2));
    let detail = usize::from(class == LayoutClass::Compact && inner_height >= 3);
    let visible = inner_height.saturating_sub(1).saturating_sub(detail).max(1);
    SessionRows { detail, visible }
}

fn session_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::plain(row.title.clone())
}

fn model_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::category(row.primary_model.clone(), CategoryKind::Model)
}

fn project_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::category(row.project.clone(), CategoryKind::Project)
}

fn agent_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::category(row.agent.clone(), CategoryKind::Agent)
}

fn agent_minutes_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::plain(row.agent_minutes.map_or_else(
        || "untimed".to_owned(),
        |value| format_count(value.round() as i128),
    ))
}

fn cost_cell(row: &SessionRow, _: Tz) -> SessionCell {
    SessionCell::plain(format_usd(row.cost))
}

fn window_cell(row: &SessionRow, timezone: Tz) -> SessionCell {
    SessionCell::plain(window(row, timezone))
}

fn window(row: &SessionRow, timezone: Tz) -> String {
    match (row.first_active, row.last_active) {
        (Some(first), Some(last)) => format_window(first, last, timezone),
        _ => "untimed".to_owned(),
    }
}

fn sort_header(app: &App, column: &ColumnSpec) -> String {
    if app.sort_column() != column.sort {
        return column.label.to_owned();
    }
    let arrow = sort_arrow(app.sort_direction());
    format!("{}{arrow}", column.label)
}

fn sort_arrow(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "↑",
        SortDirection::Descending => "↓",
    }
}

fn compose_row(marker: &str, cells: &[String], widths: &[usize]) -> String {
    let mut row = format!("{} ", pad_right(marker, 1));
    for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
        if index > 0 {
            row.push(' ');
        }
        row.push_str(&pad_right(cell, *width));
    }
    row
}

fn compose_styled_row(
    marker: &str,
    cells: &[SessionCell],
    widths: &[usize],
    colors: &CategoryColors,
    row_style: Style,
    palette: Palette,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{} ", pad_right(marker, 1)),
        row_style,
    )];
    for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
        if index > 0 {
            spans.push(Span::styled(" ", row_style));
        }
        let style = cell.category.map_or(row_style, |kind| {
            row_style.patch(colors.style(kind, &cell.text, palette))
        });
        spans.push(Span::styled(pad_right(&cell.text, *width), style));
    }
    Line::from(spans)
}

#[derive(Clone, Copy)]
enum CategoryKind {
    Model,
    Project,
    Agent,
}

struct SessionCell {
    text: String,
    category: Option<CategoryKind>,
}

impl SessionCell {
    fn plain(text: String) -> Self {
        Self {
            text,
            category: None,
        }
    }

    fn category(text: String, category: CategoryKind) -> Self {
        Self {
            text,
            category: Some(category),
        }
    }
}

struct CategoryColors {
    models: HashMap<String, usize>,
    projects: HashMap<String, usize>,
    agents: HashMap<String, usize>,
}

impl CategoryColors {
    fn new(rows: &[SessionRow]) -> Self {
        Self {
            models: assign_colors(rows.iter().map(|row| row.primary_model.as_str()), 4),
            projects: assign_colors(rows.iter().map(|row| row.project.as_str()), 0),
            agents: assign_colors(rows.iter().map(|row| row.agent.as_str()), 8),
        }
    }

    fn style(&self, kind: CategoryKind, value: &str, palette: Palette) -> Style {
        let colors = match kind {
            CategoryKind::Model => &self.models,
            CategoryKind::Project => &self.projects,
            CategoryKind::Agent => &self.agents,
        };
        colors
            .get(value)
            .map_or_else(Style::default, |index| palette.category(*index))
    }
}

fn assign_colors<'a>(
    values: impl Iterator<Item = &'a str>,
    offset: usize,
) -> HashMap<String, usize> {
    values
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(index, value)| (value.to_owned(), index + offset))
        .collect()
}

fn column_widths(width: usize, columns: &[ColumnSpec]) -> Vec<usize> {
    let mut widths = columns
        .iter()
        .map(|column| column.minimum)
        .collect::<Vec<_>>();
    let fixed = 2 + widths.len().saturating_sub(1) + widths.iter().sum::<usize>();
    let mut remaining = width.saturating_sub(fixed);
    let flexible = columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| column.flexible.then_some(index))
        .collect::<Vec<_>>();
    let mut cursor = 0;
    while remaining > 0 {
        widths[flexible[cursor % flexible.len()]] += 1;
        cursor += 1;
        remaining -= 1;
    }
    widths
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::NaiveDate;
    use chrono_tz::UTC;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render;
    use crate::app::{App, ColorMode, Focus};
    use crate::render::layout::LayoutClass;
    use crate::render::style::Palette;
    use crate::wire::{Report, ReportSelection};

    #[test]
    fn category_colors_survive_row_selection_and_repeat_by_value() {
        // If selection flattens row styling or category colors follow row position, users
        // cannot visually track a project, model, or agent while sorting and scrolling.
        let mut app = ready_app();
        app.set_focus(Focus::Sessions);
        let area = Rect::new(0, 0, 120, 6);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let project_alpha = colors_for(&buffer, "project-alpha");
        let project_beta = colors_for(&buffer, "project-beta");
        let model_alpha = colors_for(&buffer, "model-alpha");
        let model_beta = colors_for(&buffer, "model-beta");
        let codex = colors_for(&buffer, "codex");
        let reviewer = colors_for(&buffer, "reviewer");

        assert_eq!(project_alpha.len(), 1);
        assert_eq!(project_beta.len(), 1);
        assert_ne!(project_alpha[0].0, project_beta[0].0);
        assert_ne!(model_alpha[0].0, model_beta[0].0);
        assert_ne!(codex[0].0, reviewer[0].0);
        assert_eq!(codex.len(), 2);
        assert_eq!(codex[0].0, codex[1].0);
        assert_ne!(project_alpha[0].0, Color::Reset);
        assert_ne!(project_alpha[0].0, Color::Black);
        assert_ne!(project_alpha[0].1, Color::Reset);
    }

    #[test]
    fn timeline_preview_slices_sessions_by_exact_activity_intervals() {
        // If bucket preview uses the full session window or leaves the table unsliced, the
        // rows below the chart claim activity that did not occur in the inspected interval.
        let mut report: Report =
            serde_json::from_str(include_str!("../../tests/fixtures/report-v5.json")).unwrap();
        report
            .intervals
            .retain(|interval| interval.session_id == "session-alpha");
        let mut app = app_with_report(report);
        app.set_focus(Focus::Timeline);
        let area = Rect::new(0, 0, 120, 6);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let inactive = buffer_text(&buffer);
        assert!(inactive.contains("Sessions (3 total)"), "{inactive}");
        assert!(inactive.contains("Review activity view"), "{inactive}");
        assert!(inactive.contains("Imported activity"), "{inactive}");

        app.toggle_timeline_inspection();
        let mut buffer = Buffer::empty(area);
        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let preview = buffer_text(&buffer);
        assert!(preview.contains("Sessions in 13:00-13:15 (1)"), "{preview}");
        assert!(preview.contains("Implement activity view"), "{preview}");
        assert!(!preview.contains("Review activity view"), "{preview}");
        assert!(!preview.contains("Imported activity"), "{preview}");

        app.move_timeline(1);
        let mut partial_buffer = Buffer::empty(area);
        render(
            &mut partial_buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );
        let partial = buffer_text(&partial_buffer);
        assert!(partial.contains("Sessions in 13:15-13:20 (1)"), "{partial}");
        assert!(!partial.contains("Sessions in 13:15-13:30"), "{partial}");

        app.move_timeline(-1);
        app.set_focus(Focus::Sessions);
        let mut sliced_buffer = Buffer::empty(area);
        render(
            &mut sliced_buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );
        let sliced = buffer_text(&sliced_buffer);
        assert!(sliced.contains("Sessions in 13:00-13:15 (1)"), "{sliced}");
        assert!(!sliced.contains("Review activity view"), "{sliced}");

        app.toggle_timeline_inspection();
        let mut full_buffer = Buffer::empty(area);
        render(
            &mut full_buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );
        let full = buffer_text(&full_buffer);
        assert!(full.contains("Review activity view"), "{full}");
        assert!(full.contains("Imported activity"), "{full}");
    }

    #[test]
    fn bucket_slice_includes_an_instant_at_the_bucket_start() {
        // If the overlap predicate treats a zero-duration interval at the left boundary as
        // empty, its session disappears from the exact bucket where the event occurred.
        let mut report: Report =
            serde_json::from_str(include_str!("../../tests/fixtures/report-v5.json")).unwrap();
        report
            .intervals
            .retain(|interval| interval.session_id == "session-beta");
        report.intervals[0].start = report.buckets[0].start;
        report.intervals[0].end = report.buckets[0].start;
        let mut app = app_with_report(report);
        app.toggle_timeline_inspection();
        let area = Rect::new(0, 0, 120, 6);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let text = buffer_text(&buffer);
        assert!(text.contains("Review activity view"), "{text}");
    }

    #[test]
    fn wholly_future_bucket_has_a_future_title_and_no_session_rows() {
        // If a future bucket falls through to ordinary overlap filtering, the Sessions pane
        // can claim rows in a time range the report has not observed.
        let mut report: Report =
            serde_json::from_str(include_str!("../../tests/fixtures/report-v5.json")).unwrap();
        report.effective_end = report.buckets[0].end;
        report.elapsed_bucket_count = 1;
        let mut app = app_with_report(report);
        app.toggle_timeline_inspection();
        app.move_timeline(1);
        let area = Rect::new(0, 0, 120, 6);
        let mut buffer = Buffer::empty(area);

        render(
            &mut buffer,
            area,
            &app,
            LayoutClass::Medium,
            Palette::new(ColorMode::Color),
        );

        let text = buffer_text(&buffer);
        assert!(text.contains("Sessions in 13:15-13:30 (future)"), "{text}");
        assert!(!text.contains("Implement activity view"), "{text}");
        assert!(!text.contains("Review activity view"), "{text}");
    }

    fn ready_app() -> App {
        let report: Report =
            serde_json::from_str(include_str!("../../tests/fixtures/report-v5.json")).unwrap();
        app_with_report(report)
    }

    fn app_with_report(report: Report) -> App {
        let selection = ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(), UTC);
        let mut app = App::new(selection, Duration::from_secs(300));
        app.set_color_mode(ColorMode::Color);
        app.begin_foreground_load();
        app.apply_report(
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        app
    }

    fn buffer_text(buffer: &Buffer) -> String {
        (buffer.area.y..buffer.area.y + buffer.area.height)
            .map(|y| {
                (buffer.area.x..buffer.area.x + buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn colors_for(buffer: &Buffer, needle: &str) -> Vec<(Color, Color)> {
        let mut colors = Vec::new();
        for y in buffer.area.y..buffer.area.y + buffer.area.height {
            let row = (buffer.area.x..buffer.area.x + buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            let mut offset = 0;
            while let Some(found) = row[offset..].find(needle) {
                let x = buffer.area.x + (offset + found) as u16;
                let cell = &buffer[(x, y)];
                colors.push((cell.fg, cell.bg));
                offset += found + needle.len();
            }
        }
        colors
    }
}
