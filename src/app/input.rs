// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::cmp::Ordering;

use chrono::NaiveDate;

use super::{
    App, AppCommand, BreakdownCategory, CompactRegion, Focus, PopupQueryEdit, ReportState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKey {
    Quit,
    Tab,
    BackTab,
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
    Char(char),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyHint {
    pub key: &'static str,
    pub action: &'static str,
    pub(crate) compact: &'static str,
}

impl App {
    pub fn handle_input(&mut self, key: InputKey, today: NaiveDate) -> Option<AppCommand> {
        if key == InputKey::Quit
            || (key == InputKey::Char('q')
                && !self
                    .popup
                    .as_ref()
                    .is_some_and(|popup| popup.is_searchable()))
        {
            return Some(AppCommand::Quit);
        }
        if self.help_open {
            if matches!(key, InputKey::Escape | InputKey::Char('?')) {
                self.help_open = false;
            }
            return None;
        }
        if self.popup.is_some() {
            return self.handle_popup_input(key);
        }

        match key {
            InputKey::Tab => self.move_focus(1),
            InputKey::BackTab => self.move_focus(-1),
            InputKey::Char('?') => self.help_open = true,
            InputKey::Char('r') => return self.refresh_or_retry(),
            InputKey::Backspace => {
                if self.clear_focused_filter(today) {
                    return self.foreground_command();
                }
            }
            InputKey::Char('s') => {
                self.compact_region = CompactRegion::Sessions;
                self.focus = Focus::Sessions;
            }
            InputKey::Char('b') => {
                self.compact_region = CompactRegion::Breakdown;
                self.focus = Focus::Breakdowns;
            }
            InputKey::Char('p') => self.select_compact_breakdown(BreakdownCategory::Project),
            InputKey::Char('m') => self.select_compact_breakdown(BreakdownCategory::Model),
            InputKey::Char('a') => self.select_compact_breakdown(BreakdownCategory::Agent),
            _ => return self.handle_focused_input(key, today),
        }
        None
    }

    pub fn help_open(&self) -> bool {
        self.help_open
    }

    pub fn contextual_keys(&self) -> Vec<KeyHint> {
        if self.help_open {
            return vec![
                hint("Esc", "close help", "Esc close help"),
                hint("q", "close dashboard", "q quit"),
            ];
        }
        if let Some(popup) = &self.popup {
            if popup.is_searchable() {
                return vec![
                    hint("A-Z", "search", "A-Z search"),
                    hint("Backspace", "erase", "Bksp erase"),
                    hint("↑/↓", "choose", "↑/↓ choose"),
                    hint("Enter", "apply", "Enter apply"),
                    hint("Esc", "cancel", "Esc cancel"),
                ];
            }
            return vec![
                hint("↑/↓", "choose", "↑/↓ choose"),
                hint("Enter", "apply", "Enter apply"),
                hint("Esc", "cancel", "Esc cancel"),
                hint("q", "close dashboard", "q quit"),
            ];
        }
        let mut hints = match self.focus {
            Focus::Date => vec![
                hint("←/→", "day", "←/→ day"),
                hint("Backspace", "today", "Bksp today"),
            ],
            Focus::Project | Focus::Branch | Focus::Agent | Focus::Machine | Focus::Automation => {
                let mut hints = Vec::new();
                if self.focused_filter_is_ready() {
                    hints.push(hint("Enter", "choose", "Enter choose"));
                }
                hints.push(hint("Backspace", "all", "Bksp all"));
                hints
            }
            Focus::Timeline if self.timeline_inspection_active() => vec![
                hint("←/→", "bucket", "←/→ bucket"),
                hint("Enter", "all sessions", "Enter all"),
            ],
            Focus::Timeline if self.timeline_inspection_available() => {
                vec![hint("Enter", "slice sessions", "Enter slice")]
            }
            Focus::Timeline => Vec::new(),
            Focus::Sessions => vec![
                hint("←/→", "sort field", "←/→ sort"),
                hint("↑/↓", "select row", "↑/↓ row"),
                hint("Enter", "sort direction", "Enter order"),
            ],
            Focus::Breakdowns => vec![
                hint("←/→", "category", "←/→ category"),
                hint("v", "cost/time", "v cost/time"),
            ],
        };
        let retry = self.failed_metadata_for_focus().is_some()
            || matches!(
                self.report_state(),
                ReportState::Failed(_) | ReportState::Stale { .. }
            );
        hints.push(hint("Tab", "next section", "Tab next"));
        if retry {
            hints.push(hint("r", "retry", "r retry"));
        } else if !self.has_in_flight_report() {
            hints.push(hint("r", "refresh", "r refresh"));
        }
        hints.push(hint("?", "keys", "? keys"));
        hints.push(hint("q", "close dashboard", "q quit"));
        hints
    }

    fn handle_popup_input(&mut self, key: InputKey) -> Option<AppCommand> {
        let searchable = self
            .popup
            .as_ref()
            .is_some_and(|popup| popup.is_searchable());
        match key {
            InputKey::Up => self.move_popup(-1),
            InputKey::Down => self.move_popup(1),
            InputKey::Char('k') if !searchable => self.move_popup(-1),
            InputKey::Char('j') if !searchable => self.move_popup(1),
            InputKey::Enter => {
                if self.accept_popup() {
                    return self.foreground_command();
                }
            }
            InputKey::Escape => self.close_popup(),
            InputKey::Backspace if searchable => self.edit_popup_query(PopupQueryEdit::Pop),
            InputKey::Char(character) if searchable => {
                self.edit_popup_query(PopupQueryEdit::Push(character));
            }
            _ => {}
        }
        None
    }

    fn handle_focused_input(&mut self, key: InputKey, today: NaiveDate) -> Option<AppCommand> {
        match self.focus {
            Focus::Date => {
                let changed = match key {
                    InputKey::Left | InputKey::Char('h') => self.move_date(Ordering::Less),
                    InputKey::Right | InputKey::Char('l') if self.selection.date < today => {
                        self.move_date(Ordering::Greater)
                    }
                    _ => false,
                };
                if changed {
                    return self.foreground_command();
                }
            }
            Focus::Project | Focus::Branch | Focus::Agent | Focus::Machine | Focus::Automation => {
                if key == InputKey::Enter {
                    self.open_filter_popup();
                }
            }
            Focus::Timeline => match key {
                InputKey::Enter
                    if self.timeline_inspection_active()
                        || self.timeline_inspection_available() =>
                {
                    self.toggle_timeline_inspection();
                    return Some(self.session_page_command());
                }
                InputKey::Left | InputKey::Char('h') if self.timeline_inspection_active() => {
                    self.move_timeline(-1);
                    return Some(self.session_page_command());
                }
                InputKey::Right | InputKey::Char('l') if self.timeline_inspection_active() => {
                    self.move_timeline(1);
                    return Some(self.session_page_command());
                }
                _ => {}
            },
            Focus::Sessions => match key {
                InputKey::Left | InputKey::Char('h') => self.move_sort_column(-1),
                InputKey::Right | InputKey::Char('l') => self.move_sort_column(1),
                InputKey::Up | InputKey::Char('k') => self.move_selected_session(-1),
                InputKey::Down | InputKey::Char('j') => self.move_selected_session(1),
                InputKey::Enter => self.toggle_sort_direction(),
                _ => {}
            },
            Focus::Breakdowns => match key {
                InputKey::Left | InputKey::Char('h') => self.move_breakdown(-1),
                InputKey::Right | InputKey::Char('l') => self.move_breakdown(1),
                InputKey::Char('v') => self.toggle_breakdown_value(),
                _ => {}
            },
        }
        None
    }

    fn refresh_or_retry(&mut self) -> Option<AppCommand> {
        if let Some(kind) = self.failed_metadata_for_focus() {
            return Some(self.retry_metadata(kind));
        }
        if self.has_in_flight_report() {
            return None;
        }
        if let Some(request) = self.begin_refresh() {
            Some(AppCommand::FetchReport(request))
        } else {
            self.foreground_command()
        }
    }

    fn foreground_command(&mut self) -> Option<AppCommand> {
        Some(AppCommand::FetchReport(self.begin_foreground_load()))
    }

    fn session_page_command(&mut self) -> AppCommand {
        self.begin_session_page_request()
            .map_or(AppCommand::CancelSessionPage, AppCommand::FetchSessionPage)
    }

    fn select_compact_breakdown(&mut self, category: BreakdownCategory) {
        self.compact_region = CompactRegion::Breakdown;
        self.breakdown_category = category;
        self.focus = Focus::Breakdowns;
    }
}

fn hint(key: &'static str, action: &'static str, compact: &'static str) -> KeyHint {
    KeyHint {
        key,
        action,
        compact,
    }
}
