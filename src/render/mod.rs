// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

mod breakdowns;
mod chrome;
mod layout;
mod popups;
mod sessions;
mod status;
mod style;
mod summary;
mod time;
mod timeline;

use chrono::{DateTime, Utc};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::app::App;

pub use layout::{FramePlan, LayoutClass};
pub use style::{ColorMode, TerminalCapabilities};
pub use summary::format_usd;

use style::Palette;

pub fn draw(frame: &mut Frame<'_>, app: &App, plan: &FramePlan) {
    draw_frame_plan(frame.buffer_mut(), app, plan, Utc::now());
}

pub fn to_text_at(app: &App, width: u16, height: u16, now: DateTime<Utc>) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is infallible");
    terminal
        .draw(|frame| {
            let plan = FramePlan::new(app, frame.area());
            draw_frame_plan(frame.buffer_mut(), app, &plan, now);
        })
        .expect("test render is infallible");
    buffer_text(terminal.backend().buffer())
}

fn draw_frame_plan(buffer: &mut Buffer, app: &App, plan: &FramePlan, now: DateTime<Utc>) {
    let area = plan.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let palette = Palette::new(app.color_mode());
    if plan.class() == LayoutClass::TooSmall {
        render_too_small(buffer, area, palette);
        return;
    }
    if let Some(header) = plan.header() {
        chrome::render_header(buffer, header, app, plan.class(), now, palette);
    }
    if let Some(summary) = plan.summary() {
        summary::render(buffer, summary, app, plan.class(), palette);
    }
    if let Some(timeline) = plan.timeline() {
        timeline::render(buffer, timeline, app, plan.class(), palette);
    }
    if let Some(rail) = plan.compact_rail() {
        chrome::render_compact_rail(buffer, rail, app, palette);
    }
    if let Some(sessions) = plan.sessions() {
        sessions::render(buffer, sessions, app, plan.class(), palette);
    }
    if let Some(breakdowns) = plan.breakdowns() {
        breakdowns::render(buffer, breakdowns, app, plan.class(), palette);
    }
    if let Some(footer) = plan.footer() {
        chrome::render_footer(buffer, footer, app, plan.class(), palette);
    }
    popups::render(buffer, area, app, plan.class(), palette);
}

fn render_too_small(buffer: &mut Buffer, area: Rect, palette: Palette) {
    let y = area.y + area.height.saturating_sub(2) / 2;
    let notice = Paragraph::new(vec![
        Line::from(Span::styled("Need 80x24 for Activity", palette.warning())),
        Line::from(Span::styled("Resize terminal · q close", palette.muted())),
    ])
    .alignment(Alignment::Center);
    notice.render(Rect::new(area.x, y, area.width, 2.min(area.height)), buffer);
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut text = (0..buffer.area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    text
}
