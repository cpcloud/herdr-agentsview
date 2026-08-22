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

use crate::app::{App, Focus, SessionLogRow, SessionLogState};
use crate::wire::SessionLogEntry;

use super::layout::LayoutClass;
use super::status;
use super::style::{clip_with_ellipsis, pad_right, Palette};
use super::time::format_relative;

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    now: DateTime<Utc>,
    palette: Palette,
) {
    let title = match app.session_log_state() {
        SessionLogState::Loading => " Session log ".to_owned(),
        SessionLogState::Failed(_) => " Session log ".to_owned(),
        SessionLogState::Inactive => " Session log ".to_owned(),
        SessionLogState::Ready(_) => {
            format!(" Session log ({}) ", app.session_log_total().unwrap_or(0))
        }
    };
    let block = palette.block(title, app.focus() == Focus::SessionLog);
    let inner = block.inner(area);
    block.render(area, buffer);
    if render_notice(buffer, inner, app, palette) {
        return;
    }
    let inner = render_resume_notice(buffer, inner, app, palette);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let rows = app.displayed_session_log();
    if rows.is_empty() {
        let empty = format!(
            "No sessions for {} · move date or clear filters",
            app.selection().date
        );
        Paragraph::new(Line::from(Span::styled(
            clip_with_ellipsis(&empty, usize::from(inner.width)),
            palette.muted(),
        )))
        .render(inner, buffer);
        return;
    }

    let line_height = row_height(class, inner.width);
    let visible = viewport_for(inner.height, line_height);
    let scroll = app.session_log_scroll();
    let cursor = app.session_log_cursor();
    let width = usize::from(inner.width);
    let mut y = inner.y;
    for (index, row) in rows.into_iter().enumerate().skip(scroll).take(visible) {
        if y >= inner.bottom() {
            break;
        }
        let selected = index == cursor;
        let style = if selected && app.focus() == Focus::SessionLog {
            palette.session_selected()
        } else if selected {
            palette.selected()
        } else {
            Style::default()
        };
        let lines = row_lines(row, width, line_height, now, style, palette);
        for line in lines {
            if y >= inner.bottom() {
                break;
            }
            Paragraph::new(line).render(Rect::new(inner.x, y, inner.width, 1), buffer);
            y = y.saturating_add(1);
        }
    }
    if app.session_log_loading_more() && y < inner.bottom() {
        Paragraph::new(Line::from(Span::styled(
            clip_with_ellipsis("Loading more", width),
            palette.muted(),
        )))
        .render(Rect::new(inner.x, y, inner.width, 1), buffer);
    }
}

pub(super) fn viewport_rows(area: Rect, class: LayoutClass) -> usize {
    let inner_height = area.height.saturating_sub(2);
    viewport_for(
        inner_height,
        row_height(class, area.width.saturating_sub(2)),
    )
}

fn viewport_for(inner_height: u16, line_height: u16) -> usize {
    usize::from(inner_height / line_height.max(1))
}

fn row_height(class: LayoutClass, inner_width: u16) -> u16 {
    if class == LayoutClass::Compact && inner_width < 64 {
        1
    } else {
        2
    }
}

fn render_resume_notice(buffer: &mut Buffer, area: Rect, app: &App, palette: Palette) -> Rect {
    let Some(notice) = app.resume_notice() else {
        return area;
    };
    if area.height == 0 || area.width == 0 {
        return area;
    }
    let style = if app.resume_in_flight() {
        palette.muted()
    } else {
        palette.error()
    };
    Paragraph::new(Line::from(Span::styled(
        clip_with_ellipsis(notice, usize::from(area.width)),
        style,
    )))
    .render(Rect::new(area.x, area.y, area.width, 1), buffer);
    Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    )
}

fn render_notice(buffer: &mut Buffer, area: Rect, app: &App, palette: Palette) -> bool {
    let notice = match app.session_log_state() {
        SessionLogState::Loading => Some(("Loading sessions", palette.muted())),
        SessionLogState::Failed(error) => Some((error.message.as_str(), palette.error())),
        SessionLogState::Inactive | SessionLogState::Ready(_) => None,
    };
    let Some((message, style)) = notice else {
        return false;
    };
    let text = if matches!(app.session_log_state(), SessionLogState::Failed(_)) {
        format!("{message}{}", status::RECOVERY_HINT)
    } else {
        message.to_owned()
    };
    Paragraph::new(Line::from(Span::styled(
        clip_with_ellipsis(&text, usize::from(area.width)),
        style,
    )))
    .render(area, buffer);
    true
}

fn row_lines(
    row: SessionLogRow<'_>,
    width: usize,
    line_height: u16,
    now: DateTime<Utc>,
    style: Style,
    palette: Palette,
) -> Vec<Line<'static>> {
    let marker = row_marker(&row);
    let marker_width = UnicodeWidthStr::width(marker);
    let title = clip_with_ellipsis(row.entry.title(), width.saturating_sub(marker_width));
    let title_line = Line::from(Span::styled(
        pad_right(&format!("{marker}{title}"), width),
        style,
    ));
    if line_height == 1 {
        return vec![title_line];
    }
    let meta = meta_line(row.entry, row.depth, row.relationship_marker(), now, width);
    vec![
        title_line,
        Line::from(Span::styled(
            meta,
            if style == Style::default() {
                palette.muted()
            } else {
                style
            },
        )),
    ]
}

fn row_marker(row: &SessionLogRow<'_>) -> &'static str {
    if row.depth > 0 {
        return if row.depth == 1 { "  └ " } else { "    └ " };
    }
    if !row.has_children {
        "  "
    } else if row.expanded {
        "▾ "
    } else {
        "▸ "
    }
}

fn meta_line(
    entry: &SessionLogEntry,
    depth: usize,
    relationship: Option<&str>,
    now: DateTime<Utc>,
    width: usize,
) -> String {
    let indent = if depth == 0 { "  " } else { "    " };
    let recency = entry
        .recency_timestamp()
        .map(|timestamp| format_relative(now, timestamp))
        .unwrap_or_else(|| "—".to_owned());
    let project = entry.project_label().unwrap_or("—");
    let mut parts = Vec::new();
    if let Some(relationship) = relationship {
        parts.push(relationship.to_owned());
    }
    parts.push(project.to_owned());
    parts.push(recency);
    parts.push(entry.message_count.to_string());
    let agent = entry.agent_identity();
    if !agent.is_empty() {
        parts.push(agent.to_owned());
    }
    if let Some(machine) = entry.machine_label() {
        parts.push(machine.to_owned());
    }
    if entry.is_teammate {
        parts.push("team".to_owned());
    }
    let meta = format!("{indent}{}", parts.join("  "));
    pad_right(&clip_with_ellipsis(&meta, width), width)
}
