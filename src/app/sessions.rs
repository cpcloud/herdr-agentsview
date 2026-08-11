// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::wire::SessionRow;

use super::App;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSortColumn {
    Session,
    Model,
    Project,
    Agent,
    AgentMinutes,
    Cost,
    Window,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

pub(super) struct SessionState {
    column: SessionSortColumn,
    direction: SortDirection,
    cursor: usize,
    scroll: usize,
    viewport_rows: usize,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            column: SessionSortColumn::AgentMinutes,
            direction: SortDirection::Descending,
            cursor: 0,
            scroll: 0,
            viewport_rows: usize::MAX,
        }
    }
}

impl SessionState {
    pub(super) fn reset_position(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
    }

    pub(super) fn clamp(&mut self, row_count: usize) {
        if row_count == 0 {
            self.reset_position();
            return;
        }
        self.cursor = self.cursor.min(row_count - 1);
        self.clamp_scroll(row_count);
    }

    fn clamp_scroll(&mut self, row_count: usize) {
        self.scroll = self.scroll_for(row_count, self.viewport_rows);
    }

    fn scroll_for(&self, row_count: usize, visible_rows: usize) -> usize {
        if row_count == 0 {
            return 0;
        }
        let cursor = self.cursor.min(row_count - 1);
        let visible = visible_rows.max(1).min(row_count);
        let mut scroll = self.scroll.min(row_count - visible);
        if cursor < scroll {
            scroll = cursor;
        } else if cursor >= scroll.saturating_add(visible) {
            scroll = cursor + 1 - visible;
        }
        scroll.min(row_count - visible)
    }
}

impl App {
    pub fn sort_column(&self) -> SessionSortColumn {
        self.sessions.column
    }

    pub fn sort_direction(&self) -> SortDirection {
        self.sessions.direction
    }

    pub fn toggle_sort_direction(&mut self) {
        let selected_session_id = self.selected_session_id();
        self.sessions.direction = match self.sessions.direction {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        };
        self.restore_session_selection(selected_session_id.as_deref());
    }

    pub fn sorted_sessions(&self) -> Vec<&SessionRow> {
        let Some(report) = self.report() else {
            return Vec::new();
        };
        let mut rows: Vec<_> = report.by_session.iter().collect();
        rows.sort_by(|left, right| {
            compare_rows(left, right, self.sessions.column, self.sessions.direction)
        });
        rows
    }

    pub(crate) fn displayed_sessions(&self) -> Vec<&SessionRow> {
        let mut rows = self.sorted_sessions();
        let (Some(report), Some(bucket)) = (self.report(), self.inspected_bucket()) else {
            return rows;
        };
        if report.bucket_is_future(bucket) {
            rows.clear();
            return rows;
        }
        let observed_end = report.observed_bucket_end(bucket);
        let active_sessions = report
            .intervals
            .iter()
            .filter(|interval| {
                if interval.start == interval.end {
                    bucket.start <= interval.start && interval.start < observed_end
                } else {
                    interval.start < observed_end && interval.end > bucket.start
                }
            })
            .map(|interval| interval.session_id.as_str())
            .collect::<BTreeSet<_>>();
        rows.retain(|row| active_sessions.contains(row.session_id.as_str()));
        rows
    }

    pub fn session_cursor(&self) -> usize {
        self.sessions.cursor
    }

    pub fn session_scroll(&self) -> usize {
        self.sessions.scroll
    }

    pub(crate) fn session_scroll_for_viewport(
        &self,
        row_count: usize,
        visible_rows: usize,
    ) -> usize {
        self.sessions.scroll_for(row_count, visible_rows)
    }

    pub fn move_session(&mut self, delta: isize, visible_rows: usize) {
        let row_count = self.displayed_sessions().len();
        self.sessions.viewport_rows = visible_rows.max(1);
        if row_count == 0 {
            self.sessions.reset_position();
            return;
        }
        self.sessions.cursor = self
            .sessions
            .cursor
            .saturating_add_signed(delta)
            .min(row_count - 1);
        self.sessions.clamp_scroll(row_count);
    }

    pub fn set_session_viewport_rows(&mut self, visible_rows: usize) {
        self.sessions.viewport_rows = visible_rows.max(1);
        let row_count = self.displayed_sessions().len();
        self.sessions.clamp(row_count);
    }

    pub(crate) fn move_selected_session(&mut self, delta: isize) {
        self.move_session(delta, self.sessions.viewport_rows);
    }

    pub(crate) fn move_sort_column(&mut self, delta: isize) {
        const COLUMNS: [SessionSortColumn; 7] = [
            SessionSortColumn::Session,
            SessionSortColumn::Model,
            SessionSortColumn::Project,
            SessionSortColumn::Agent,
            SessionSortColumn::AgentMinutes,
            SessionSortColumn::Cost,
            SessionSortColumn::Window,
        ];
        let selected_session_id = self.selected_session_id();
        let current = COLUMNS
            .iter()
            .position(|column| *column == self.sessions.column)
            .expect("closed session sort column");
        let next = (current as isize + delta).rem_euclid(COLUMNS.len() as isize) as usize;
        self.sessions.column = COLUMNS[next];
        self.restore_session_selection(selected_session_id.as_deref());
    }

    pub(super) fn selected_session_id(&self) -> Option<String> {
        self.displayed_sessions()
            .get(self.sessions.cursor)
            .map(|row| row.session_id.clone())
    }

    pub(super) fn restore_session_selection(&mut self, selected_session_id: Option<&str>) {
        let rows = self.displayed_sessions();
        let cursor = selected_session_id.and_then(|selected_session_id| {
            rows.iter()
                .position(|row| row.session_id == selected_session_id)
        });
        let row_count = rows.len();
        if let Some(cursor) = cursor {
            self.sessions.cursor = cursor;
        }
        self.sessions.clamp(row_count);
    }

    pub(super) fn restore_session_selection_or_first(&mut self, selected_session_id: Option<&str>) {
        let (cursor, row_count) = {
            let rows = self.displayed_sessions();
            let cursor = selected_session_id
                .and_then(|selected_session_id| {
                    rows.iter()
                        .position(|row| row.session_id == selected_session_id)
                })
                .unwrap_or(0);
            (cursor, rows.len())
        };
        self.sessions.cursor = cursor;
        self.sessions.clamp(row_count);
    }
}

fn compare_rows(
    left: &SessionRow,
    right: &SessionRow,
    column: SessionSortColumn,
    direction: SortDirection,
) -> Ordering {
    let primary = match column {
        SessionSortColumn::Session => directional(left.title.cmp(&right.title), direction),
        SessionSortColumn::Model => {
            directional(left.primary_model.cmp(&right.primary_model), direction)
        }
        SessionSortColumn::Project => directional(left.project.cmp(&right.project), direction),
        SessionSortColumn::Agent => directional(left.agent.cmp(&right.agent), direction),
        SessionSortColumn::AgentMinutes => compare_nullable(
            left.agent_minutes.as_ref(),
            right.agent_minutes.as_ref(),
            direction,
            |left, right| left.total_cmp(right),
        ),
        SessionSortColumn::Cost => directional(
            left.cost.microdollars.cmp(&right.cost.microdollars),
            direction,
        ),
        SessionSortColumn::Window => compare_nullable(
            left.first_active.as_ref().zip(left.last_active.as_ref()),
            right.first_active.as_ref().zip(right.last_active.as_ref()),
            direction,
            |left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)),
        ),
    };
    primary.then_with(|| left.session_id.cmp(&right.session_id))
}

fn compare_nullable<T>(
    left: Option<T>,
    right: Option<T>,
    direction: SortDirection,
    compare: impl FnOnce(T, T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => directional(compare(left, right), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn directional(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}
