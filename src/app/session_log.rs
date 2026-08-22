// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, BTreeSet};

use crate::api::ApiError;
use crate::wire::{Automation, SessionLogEntry, SessionLogPage, SessionLogQuery};

use super::{App, AppCommand, Focus, View};

#[derive(Clone, Debug, PartialEq)]
pub enum SessionLogState {
    Inactive,
    Loading,
    Ready(SessionLogData),
    Failed(ApiError),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionLogData {
    pub sessions: Vec<SessionLogEntry>,
    pub next_cursor: Option<String>,
    pub total: usize,
    pub query: SessionLogQuery,
    cursor: usize,
    scroll: usize,
    viewport_rows: usize,
    expanded: BTreeSet<String>,
    loading_more: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionLogRow<'a> {
    pub entry: &'a SessionLogEntry,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

impl SessionLogRow<'_> {
    pub fn relationship_marker(&self) -> Option<&str> {
        if self.depth == 0 {
            return None;
        }
        nonempty(self.entry.relationship_type.as_deref())
    }
}

impl SessionLogData {
    fn from_page(page: SessionLogPage, query: SessionLogQuery) -> Self {
        let next_cursor = nonempty(page.next_cursor.as_deref()).map(ToOwned::to_owned);
        Self {
            sessions: page.sessions,
            next_cursor,
            total: page.total,
            query,
            cursor: 0,
            scroll: 0,
            viewport_rows: usize::MAX,
            expanded: BTreeSet::new(),
            loading_more: false,
        }
    }

    fn append_page(&mut self, page: SessionLogPage) {
        self.sessions.extend(page.sessions);
        self.next_cursor = nonempty(page.next_cursor.as_deref()).map(ToOwned::to_owned);
        self.total = page.total;
        self.loading_more = false;
    }

    fn visible(&self, automation: Automation) -> Vec<SessionLogRow<'_>> {
        visible_rows(&self.sessions, automation, &self.expanded)
    }

    fn clamp(&mut self, row_count: usize) {
        if row_count == 0 {
            self.cursor = 0;
            self.scroll = 0;
            return;
        }
        self.cursor = self.cursor.min(row_count - 1);
        self.clamp_scroll(row_count);
    }

    fn clamp_scroll(&mut self, row_count: usize) {
        self.scroll = scroll_for(self.cursor, self.scroll, row_count, self.viewport_rows);
    }
}

impl App {
    pub fn view(&self) -> View {
        self.view
    }

    pub fn session_log_open(&self) -> bool {
        self.view == View::SessionLog
    }

    pub fn session_log_state(&self) -> &SessionLogState {
        &self.session_log
    }

    pub fn session_log_is_loading(&self) -> bool {
        matches!(self.session_log, SessionLogState::Loading)
            || matches!(
                &self.session_log,
                SessionLogState::Ready(data) if data.loading_more
            )
    }

    pub fn session_log_query(&self) -> SessionLogQuery {
        SessionLogQuery::from_selection(&self.selection)
    }

    pub fn displayed_session_log(&self) -> Vec<SessionLogRow<'_>> {
        match &self.session_log {
            SessionLogState::Ready(data) => data.visible(self.selection.automation),
            SessionLogState::Inactive | SessionLogState::Loading | SessionLogState::Failed(_) => {
                Vec::new()
            }
        }
    }

    pub fn session_log_cursor(&self) -> usize {
        match &self.session_log {
            SessionLogState::Ready(data) => data.cursor,
            _ => 0,
        }
    }

    pub fn session_log_scroll(&self) -> usize {
        match &self.session_log {
            SessionLogState::Ready(data) => data.scroll,
            _ => 0,
        }
    }

    pub fn session_log_loading_more(&self) -> bool {
        matches!(
            &self.session_log,
            SessionLogState::Ready(data) if data.loading_more
        )
    }

    pub fn session_log_total(&self) -> Option<usize> {
        match &self.session_log {
            SessionLogState::Ready(data) => Some(data.total),
            _ => None,
        }
    }

    pub fn set_session_log_viewport_rows(&mut self, visible_rows: usize) {
        let automation = self.selection.automation;
        if let SessionLogState::Ready(data) = &mut self.session_log {
            data.viewport_rows = visible_rows.max(1);
            let row_count = data.visible(automation).len();
            data.clamp(row_count);
        }
    }

    pub fn apply_session_log(&mut self, result: Result<SessionLogPage, ApiError>, append: bool) {
        let query = self.session_log_query();
        match (
            append,
            result,
            std::mem::replace(&mut self.session_log, SessionLogState::Inactive),
        ) {
            (true, Ok(page), SessionLogState::Ready(mut data)) => {
                data.append_page(page);
                let row_count = data.visible(self.selection.automation).len();
                data.clamp(row_count);
                self.session_log = SessionLogState::Ready(data);
            }
            (_, Ok(page), previous) => {
                let mut data = SessionLogData::from_page(page, query);
                if let SessionLogState::Ready(previous) = previous {
                    data.expanded = previous.expanded;
                    data.viewport_rows = previous.viewport_rows;
                    data.cursor = previous.cursor;
                    data.scroll = previous.scroll;
                }
                let row_count = data.visible(self.selection.automation).len();
                data.clamp(row_count);
                self.session_log = SessionLogState::Ready(data);
            }
            (true, Err(_), SessionLogState::Ready(mut data)) => {
                data.loading_more = false;
                self.session_log = SessionLogState::Ready(data);
            }
            (_, Err(error), _) => {
                self.session_log = SessionLogState::Failed(error);
            }
        }
    }

    pub(crate) fn open_or_toggle_session_log(&mut self) -> Option<AppCommand> {
        if self.session_log_open() {
            self.close_session_log();
            return None;
        }
        self.open_session_log()
    }

    pub(crate) fn open_session_log(&mut self) -> Option<AppCommand> {
        if self.view != View::SessionLog {
            self.activity_focus = self.focus;
        }
        self.view = View::SessionLog;
        self.focus = Focus::SessionLog;
        self.popup = None;
        if self.session_log_cache_matches() {
            return None;
        }
        self.begin_session_log_load(false)
    }

    pub(crate) fn close_session_log(&mut self) {
        if self.view != View::SessionLog {
            return;
        }
        self.view = View::Activity;
        self.focus = self.activity_focus;
        self.popup = None;
    }

    pub(crate) fn begin_session_log_load(&mut self, append: bool) -> Option<AppCommand> {
        let mut query = self.session_log_query();
        if append {
            let SessionLogState::Ready(data) = &mut self.session_log else {
                return None;
            };
            let cursor = data.next_cursor.clone()?;
            if data.loading_more {
                return None;
            }
            data.loading_more = true;
            query = query.with_cursor(cursor);
            return Some(AppCommand::FetchSessionLog {
                query,
                append: true,
            });
        }
        self.session_log = SessionLogState::Loading;
        Some(AppCommand::FetchSessionLog {
            query,
            append: false,
        })
    }

    pub(crate) fn move_session_log(&mut self, delta: isize) -> Option<AppCommand> {
        let automation = self.selection.automation;
        let page_more = {
            let data = self.ready_session_log_mut()?;
            let row_count = data.visible(automation).len();
            if row_count == 0 {
                data.reset_position();
                return None;
            }
            let at_end = data.cursor + 1 >= row_count && delta > 0;
            data.cursor = data
                .cursor
                .saturating_add_signed(delta)
                .min(row_count.saturating_sub(1));
            data.clamp_scroll(row_count);
            at_end
        };
        if page_more {
            self.begin_session_log_load(true)
        } else {
            None
        }
    }

    pub(crate) fn toggle_session_log_expand(&mut self) {
        self.set_selected_session_log_expanded(None);
    }

    pub(crate) fn collapse_selected_session_log(&mut self) {
        self.set_selected_session_log_expanded(Some(false));
    }

    pub(crate) fn expand_selected_session_log(&mut self) {
        self.set_selected_session_log_expanded(Some(true));
    }

    fn set_selected_session_log_expanded(&mut self, expand: Option<bool>) {
        let selected = self
            .displayed_session_log()
            .get(self.session_log_cursor())
            .filter(|row| row.has_children)
            .map(|row| (row.entry.id.clone(), row.expanded));
        let Some((selected_id, is_expanded)) = selected else {
            return;
        };
        let should_expand = expand.unwrap_or(!is_expanded);
        if should_expand == is_expanded {
            return;
        }
        let automation = self.selection.automation;
        let Some(data) = self.ready_session_log_mut() else {
            return;
        };
        if should_expand {
            data.expanded.insert(selected_id);
        } else {
            data.expanded.remove(&selected_id);
        }
        let row_count = data.visible(automation).len();
        data.clamp(row_count);
    }

    pub(crate) fn invalidate_session_log(&mut self) {
        if self.session_log_open() {
            self.session_log = SessionLogState::Loading;
        } else {
            self.session_log = SessionLogState::Inactive;
        }
    }

    fn session_log_cache_matches(&self) -> bool {
        match &self.session_log {
            SessionLogState::Ready(data) => {
                let current = self.session_log_query();
                data.query.date == current.date
                    && data.query.timezone == current.timezone
                    && data.query.project == current.project
                    && data.query.agent == current.agent
                    && data.query.machine == current.machine
                    && data.query.include_automated == current.include_automated
            }
            _ => false,
        }
    }

    fn ready_session_log_mut(&mut self) -> Option<&mut SessionLogData> {
        match &mut self.session_log {
            SessionLogState::Ready(data) => Some(data),
            _ => None,
        }
    }
}

impl SessionLogData {
    fn reset_position(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
    }
}

fn visible_rows<'a>(
    sessions: &'a [SessionLogEntry],
    automation: Automation,
    expanded: &BTreeSet<String>,
) -> Vec<SessionLogRow<'a>> {
    let entries = sessions
        .iter()
        .filter(|entry| automation != Automation::Automated || entry.is_automated)
        .collect::<Vec<_>>();
    let ids = entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut children: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut roots = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match nonempty(entry.parent_session_id.as_deref()) {
            Some(parent) if ids.contains(parent) => {
                children.entry(parent).or_default().push(index);
            }
            _ => roots.push(index),
        }
    }
    let mut rows = Vec::with_capacity(entries.len());
    for root in roots {
        push_visible(&mut rows, &entries, &children, expanded, root, 0);
    }
    rows
}

fn push_visible<'a>(
    rows: &mut Vec<SessionLogRow<'a>>,
    entries: &[&'a SessionLogEntry],
    children: &BTreeMap<&str, Vec<usize>>,
    expanded: &BTreeSet<String>,
    index: usize,
    depth: usize,
) {
    let entry = entries[index];
    let child_indexes = children
        .get(entry.id.as_str())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let is_expanded = expanded.contains(&entry.id);
    rows.push(SessionLogRow {
        entry,
        depth,
        has_children: !child_indexes.is_empty(),
        expanded: is_expanded,
    });
    if is_expanded {
        for child in child_indexes {
            push_visible(rows, entries, children, expanded, *child, depth + 1);
        }
    }
}

fn scroll_for(cursor: usize, scroll: usize, row_count: usize, visible_rows: usize) -> usize {
    if row_count == 0 {
        return 0;
    }
    let cursor = cursor.min(row_count - 1);
    let visible = visible_rows.max(1).min(row_count);
    let mut scroll = scroll.min(row_count - visible);
    if cursor < scroll {
        scroll = cursor;
    } else if cursor >= scroll.saturating_add(visible) {
        scroll = cursor + 1 - visible;
    }
    scroll.min(row_count - visible)
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then_some(value)
    })
}
