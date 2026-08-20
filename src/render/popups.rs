// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus};

use super::chrome;
use super::layout::LayoutClass;
use super::style::{clip_with_ellipsis, Palette};

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    if app.help_open() {
        render_help(buffer, area, palette);
    } else if let Some(popup) = app.popup() {
        let searchable = popup.is_searchable();
        let max_label = popup
            .labels()
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0);
        let query_width = UnicodeWidthStr::width(popup.query.as_str()) + 12;
        let Some((anchor_x, selector_width)) =
            chrome::filter_anchor(app, class, usize::from(area.width), app.focus())
        else {
            return;
        };
        let width = (max_label.max(query_width) as u16 + 6)
            .max(selector_width)
            .clamp(24, area.width);
        let content_height = popup.len() as u16 + u16::from(searchable);
        let height = (content_height + 2)
            .max(3 + u16::from(searchable))
            .min(area.height.saturating_sub(3));
        let x = (area.x + anchor_x).min(area.right().saturating_sub(width));
        let popup_area = Rect::new(x, area.y + 2, width, height);
        Clear.render(popup_area, buffer);
        let block = palette.block(format!(" {} ", focus_name(app.focus())), true);
        let mut inner = block.inner(popup_area);
        block.render(popup_area, buffer);
        if searchable && inner.height > 0 {
            let query = if popup.query.is_empty() {
                "Search: type to filter".to_owned()
            } else {
                format!("Search: {}_", popup.query)
            };
            Paragraph::new(Line::from(Span::styled(
                clip_with_ellipsis(&query, usize::from(inner.width)),
                if popup.query.is_empty() {
                    palette.muted()
                } else {
                    ratatui::style::Style::default()
                },
            )))
            .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
            inner.y += 1;
            inner.height = inner.height.saturating_sub(1);
        }
        let visible = usize::from(inner.height);
        let start = popup
            .selected
            .saturating_sub(visible / 2)
            .min(popup.len().saturating_sub(visible));
        let lines = if popup.is_empty() {
            vec![Line::from(Span::styled(
                "No matching projects",
                palette.muted(),
            ))]
        } else {
            popup
                .labels()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(index, label)| {
                    let selected = index == popup.selected;
                    let text = format!("  {label}");
                    let style = if selected {
                        palette.selected()
                    } else {
                        ratatui::style::Style::default()
                    };
                    Line::from(Span::styled(
                        clip_with_ellipsis(&text, usize::from(inner.width)),
                        style,
                    ))
                })
                .collect::<Vec<_>>()
        };
        Paragraph::new(lines).render(inner, buffer);
    }
}

fn render_help(buffer: &mut Buffer, area: Rect, palette: Palette) {
    let width = 70.min(area.width.saturating_sub(4));
    let height = 18.min(area.height.saturating_sub(4));
    let popup_area = centered(area, width, height);
    Clear.render(popup_area, buffer);
    let block = palette.block(" Keyboard help ", true);
    let inner = block.inner(popup_area);
    block.render(popup_area, buffer);
    let rows = [
        HelpRow::Heading("NAVIGATION"),
        HelpRow::Pair(("Tab", "next section"), ("Shift-Tab", "previous section")),
        HelpRow::Pair(("←/→", "change / inspect"), ("↑/↓", "move selection")),
        HelpRow::Pair(("Enter", "choose / slice"), ("Backspace", "clear / today")),
        HelpRow::Blank,
        HelpRow::Heading("BREAKDOWNS"),
        HelpRow::Pair(("v", "cost ↔ time"), ("p/m/a", "project / model / agent")),
        HelpRow::Blank,
        HelpRow::Heading("COMPACT VIEW"),
        HelpRow::Pair(("s", "sessions"), ("b", "breakdowns")),
        HelpRow::Blank,
        HelpRow::Heading("GENERAL"),
        HelpRow::Pair(("r", "refresh / retry"), ("Esc", "cancel / close")),
        HelpRow::Pair(("?", "close help"), ("q", "close dashboard")),
    ];
    for (offset, row) in rows.into_iter().enumerate().take(usize::from(inner.height)) {
        render_help_row(
            buffer,
            Rect::new(inner.x, inner.y + offset as u16, inner.width, 1),
            row,
            palette,
        );
    }
}

#[derive(Clone, Copy)]
enum HelpRow {
    Heading(&'static str),
    Pair((&'static str, &'static str), (&'static str, &'static str)),
    Blank,
}

fn render_help_row(buffer: &mut Buffer, area: Rect, row: HelpRow, palette: Palette) {
    match row {
        HelpRow::Heading(label) => {
            Paragraph::new(Line::from(Span::styled(label, palette.muted()))).render(area, buffer);
        }
        HelpRow::Pair(left, right) => {
            let left_width = area.width / 2;
            render_help_hint(
                buffer,
                Rect::new(area.x, area.y, left_width, 1),
                left,
                palette,
            );
            render_help_hint(
                buffer,
                Rect::new(
                    area.x + left_width,
                    area.y,
                    area.width.saturating_sub(left_width),
                    1,
                ),
                right,
                palette,
            );
        }
        HelpRow::Blank => {}
    }
}

fn render_help_hint(
    buffer: &mut Buffer,
    area: Rect,
    (key, action): (&str, &str),
    palette: Palette,
) {
    let key_width = UnicodeWidthStr::width(key) + 2;
    let action_width = usize::from(area.width).saturating_sub(key_width + 1);
    Paragraph::new(Line::from(vec![
        Span::styled(format!(" {key} "), palette.keycap()),
        Span::raw(" "),
        Span::raw(clip_with_ellipsis(action, action_width)),
    ]))
    .render(area, buffer);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

fn focus_name(focus: Focus) -> &'static str {
    match focus {
        Focus::Project => "Project",
        Focus::Branch => "Branch",
        Focus::Agent => "Agent",
        Focus::Machine => "Machine",
        Focus::Automation => "Session category",
        Focus::Date => "Date",
        Focus::Timeline => "Timeline",
        Focus::Sessions => "Sessions",
        Focus::Breakdowns => "Breakdowns",
    }
}
