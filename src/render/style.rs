// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub use crate::app::ColorMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub color_count: u16,
    pub no_color: bool,
    pub term_is_dumb: bool,
}

impl TerminalCapabilities {
    pub fn color_mode(self) -> ColorMode {
        if self.no_color || self.term_is_dumb || self.color_count <= 2 {
            ColorMode::Monochrome
        } else {
            ColorMode::Color
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Palette {
    mode: ColorMode,
}

impl Palette {
    pub(super) fn new(mode: ColorMode) -> Self {
        Self { mode }
    }

    pub(super) fn interactive(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default().fg(Color::LightBlue),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn automated(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default().fg(Color::LightYellow),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::DIM),
        }
    }

    pub(super) fn mixed_activity(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default().fg(Color::LightBlue).bg(Color::LightYellow),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn focus(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn selected(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::Black)
                .bg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn session_selected(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn category(self, index: usize) -> Style {
        const COLORS: [Color; 10] = [
            Color::LightCyan,
            Color::LightYellow,
            Color::LightMagenta,
            Color::LightGreen,
            Color::LightBlue,
            Color::LightRed,
            Color::Cyan,
            Color::Yellow,
            Color::Magenta,
            Color::Green,
        ];
        match self.mode {
            ColorMode::Color => Style::default().fg(COLORS[index % COLORS.len()]),
            ColorMode::Monochrome => Style::default(),
        }
    }

    pub(super) fn keycap(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn timeline_cursor(self, style: Style) -> Style {
        match self.mode {
            ColorMode::Color => style
                .fg(Color::Gray)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => style.add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn muted(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default().fg(Color::DarkGray),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::DIM),
        }
    }

    pub(super) fn warning(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default().add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn focused_warning(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn error(self) -> Style {
        match self.mode {
            ColorMode::Color => Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
            ColorMode::Monochrome => Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED),
        }
    }

    pub(super) fn block(self, title: impl Into<String>, focused: bool) -> Block<'static> {
        let style = if focused { self.focus() } else { self.muted() };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(style)
            .title(title.into())
    }

    pub(super) fn mode(self) -> ColorMode {
        self.mode
    }
}

pub(super) fn clip(value: &str, max_width: usize) -> String {
    let mut width = 0;
    value
        .graphemes(true)
        .take_while(|grapheme| {
            let grapheme_width = UnicodeWidthStr::width(*grapheme);
            if width + grapheme_width > max_width {
                false
            } else {
                width += grapheme_width;
                true
            }
        })
        .collect()
}

pub(super) fn clip_with_ellipsis(value: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    format!("{}…", clip(value, max_width - 1))
}

pub(super) fn pad_right(value: &str, width: usize) -> String {
    let value = clip_with_ellipsis(value, width);
    let padding = width.saturating_sub(UnicodeWidthStr::width(value.as_str()));
    format!("{value}{}", " ".repeat(padding))
}

pub(super) fn fit_with_right(left: &str, right: &str, width: usize) -> String {
    let right_width = UnicodeWidthStr::width(right);
    if right_width >= width {
        return clip_with_ellipsis(right, width);
    }
    let left = clip_with_ellipsis(left, width - right_width);
    let (left, gap, right) = align_fitted_parts(left, right.to_owned(), width);
    format!("{left}{}{right}", " ".repeat(gap))
}

fn align_fitted_parts(left: String, right: String, width: usize) -> (String, usize, String) {
    let gap = width
        .saturating_sub(UnicodeWidthStr::width(left.as_str()))
        .saturating_sub(UnicodeWidthStr::width(right.as_str()));
    (left, gap, right)
}

#[cfg(test)]
mod tests {
    use super::clip_with_ellipsis;

    #[test]
    fn clipping_keeps_extended_grapheme_clusters_intact() {
        // If clipping iterates scalar characters, a ZWJ emoji can end in a dangling joiner and
        // render as a broken partial glyph even though the cluster fits beside the ellipsis.
        assert_eq!(clip_with_ellipsis("👩‍💻-project", 3), "👩‍💻…");
    }
}
