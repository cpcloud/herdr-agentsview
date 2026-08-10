mod filters;
mod input;
mod sessions;

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::api::{ApiError, ApiErrorKind};
use crate::wire::{AgentInfo, KeyMinutes, ProjectInfo, Report, ReportSelection};

pub(crate) use filters::PopupQueryEdit;
pub use filters::{CompactRegion, FilterPopup, Focus, MetadataKind};
pub use input::{InputKey, KeyHint};
pub use sessions::{SessionSortColumn, SortDirection};

use sessions::SessionState;

/// Controls whether a request keeps the last good report mounted while it loads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    Foreground,
    Refresh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReportRequest {
    pub generation: u64,
    pub kind: RequestKind,
    pub selection: ReportSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppCommand {
    FetchReport(ReportRequest),
    FetchMetadata(MetadataKind),
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReportState {
    InitialLoading {
        generation: Option<u64>,
    },
    Ready {
        report: Box<Report>,
        received_at: DateTime<Utc>,
    },
    Refreshing {
        generation: u64,
        report: Box<Report>,
        received_at: DateTime<Utc>,
    },
    Stale {
        report: Box<Report>,
        received_at: DateTime<Utc>,
        error: ApiError,
    },
    Failed(ApiError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(ApiError),
}

#[derive(Debug)]
pub enum RuntimeEvent {
    Report {
        generation: u64,
        result: Result<Box<Report>, ApiError>,
        received_at: DateTime<Utc>,
    },
    Projects(Result<Vec<ProjectInfo>, ApiError>),
    Agents(Result<Vec<AgentInfo>, ApiError>),
    Machines(Result<Vec<String>, ApiError>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BreakdownCategory {
    Project,
    Model,
    Agent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BreakdownValue {
    AgentMinutes,
    Cost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    Color,
    Monochrome,
}

pub struct App {
    selection: ReportSelection,
    refresh_interval: Duration,
    generation_counter: u64,
    report_state: ReportState,
    projects: Loadable<Vec<ProjectInfo>>,
    agents: Loadable<Vec<AgentInfo>>,
    machines: Loadable<Vec<String>>,
    focus: Focus,
    popup: Option<FilterPopup>,
    help_open: bool,
    sessions: SessionState,
    timeline_cursor: usize,
    timeline_inspection_active: bool,
    breakdown_category: BreakdownCategory,
    breakdown_value: BreakdownValue,
    compact_region: CompactRegion,
    color_mode: ColorMode,
}

impl App {
    pub fn new(selection: ReportSelection, refresh_interval: Duration) -> Self {
        Self {
            selection,
            refresh_interval,
            generation_counter: 0,
            report_state: ReportState::InitialLoading { generation: None },
            projects: Loadable::Loading,
            agents: Loadable::Loading,
            machines: Loadable::Loading,
            focus: Focus::Date,
            popup: None,
            help_open: false,
            sessions: SessionState::default(),
            timeline_cursor: 0,
            timeline_inspection_active: false,
            breakdown_category: BreakdownCategory::Project,
            breakdown_value: BreakdownValue::AgentMinutes,
            compact_region: CompactRegion::Sessions,
            color_mode: ColorMode::Color,
        }
    }

    pub fn selection(&self) -> &ReportSelection {
        &self.selection
    }

    pub fn refresh_interval(&self) -> Duration {
        self.refresh_interval
    }

    pub fn report_state(&self) -> &ReportState {
        &self.report_state
    }

    pub fn report(&self) -> Option<&Report> {
        match &self.report_state {
            ReportState::Ready { report, .. }
            | ReportState::Refreshing { report, .. }
            | ReportState::Stale { report, .. } => Some(report),
            ReportState::InitialLoading { .. } | ReportState::Failed(_) => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.report()
            .is_some_and(|report| report.totals.sessions == 0)
    }

    pub fn begin_foreground_load(&mut self) -> ReportRequest {
        let generation = self.next_generation();
        self.report_state = ReportState::InitialLoading {
            generation: Some(generation),
        };
        self.timeline_cursor = 0;
        self.timeline_inspection_active = false;
        self.sessions.reset_position();
        ReportRequest {
            generation,
            kind: RequestKind::Foreground,
            selection: self.selection.clone(),
        }
    }

    pub fn begin_refresh(&mut self) -> Option<ReportRequest> {
        let previous = std::mem::replace(
            &mut self.report_state,
            ReportState::InitialLoading { generation: None },
        );
        let (report, received_at) = match previous {
            ReportState::Ready {
                report,
                received_at,
            }
            | ReportState::Stale {
                report,
                received_at,
                ..
            } => (report, received_at),
            state => {
                self.report_state = state;
                return None;
            }
        };
        let generation = self.next_generation();
        self.report_state = ReportState::Refreshing {
            generation,
            report,
            received_at,
        };
        Some(ReportRequest {
            generation,
            kind: RequestKind::Refresh,
            selection: self.selection.clone(),
        })
    }

    pub fn begin_scheduled_load(&mut self) -> Option<ReportRequest> {
        let retry_transient_failure = matches!(
            self.report_state,
            ReportState::Failed(ApiError {
                kind: ApiErrorKind::Timeout | ApiErrorKind::Network | ApiErrorKind::Server,
                ..
            })
        );
        if retry_transient_failure {
            return Some(self.begin_foreground_load());
        }
        self.begin_refresh()
    }

    pub fn supersede_pending_load(&mut self) -> Option<ReportRequest> {
        let previous = std::mem::replace(
            &mut self.report_state,
            ReportState::InitialLoading { generation: None },
        );
        let (generation, kind) = match previous {
            ReportState::InitialLoading {
                generation: Some(_),
            } => {
                let generation = self.next_generation();
                self.report_state = ReportState::InitialLoading {
                    generation: Some(generation),
                };
                (generation, RequestKind::Foreground)
            }
            ReportState::Refreshing {
                report,
                received_at,
                ..
            } => {
                let generation = self.next_generation();
                self.report_state = ReportState::Refreshing {
                    generation,
                    report,
                    received_at,
                };
                (generation, RequestKind::Refresh)
            }
            state => {
                self.report_state = state;
                return None;
            }
        };
        Some(ReportRequest {
            generation,
            kind,
            selection: self.selection.clone(),
        })
    }

    pub fn apply_report(
        &mut self,
        generation: u64,
        result: Result<Box<Report>, ApiError>,
        received_at: DateTime<Utc>,
    ) {
        if self.pending_generation() != Some(generation) {
            return;
        }
        let selected_session_id = match &result {
            Ok(_) => self.selected_session_id(),
            Err(_) => None,
        };
        let previous = std::mem::replace(
            &mut self.report_state,
            ReportState::InitialLoading { generation: None },
        );
        let initial_load = matches!(&previous, ReportState::InitialLoading { .. });
        match (previous, result) {
            (ReportState::InitialLoading { .. }, Ok(report))
            | (ReportState::Refreshing { .. }, Ok(report)) => {
                if self.timeline_inspection_active && self.timeline_cursor >= report.buckets.len() {
                    self.timeline_inspection_active = false;
                }
                let last = report.buckets.len().saturating_sub(1);
                self.timeline_cursor = if initial_load {
                    report.first_activity_bucket_index()
                } else {
                    self.timeline_cursor.min(last)
                };
                self.report_state = ReportState::Ready {
                    report,
                    received_at,
                };
                self.restore_session_selection(selected_session_id.as_deref());
            }
            (
                ReportState::Refreshing {
                    report,
                    received_at,
                    ..
                },
                Err(error),
            ) => {
                self.report_state = ReportState::Stale {
                    report,
                    received_at,
                    error,
                };
            }
            (ReportState::InitialLoading { .. }, Err(error)) => {
                self.report_state = ReportState::Failed(error);
            }
            _ => unreachable!("pending generation belongs to a loading report state"),
        }
    }

    pub fn apply_event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Report {
                generation,
                result,
                received_at,
            } => self.apply_report(generation, result, received_at),
            RuntimeEvent::Projects(result) => self.projects = result.into(),
            RuntimeEvent::Agents(result) => self.agents = result.into(),
            RuntimeEvent::Machines(result) => self.machines = result.into(),
        }
    }

    pub fn projects(&self) -> &Loadable<Vec<ProjectInfo>> {
        &self.projects
    }

    pub fn agents(&self) -> &Loadable<Vec<AgentInfo>> {
        &self.agents
    }

    pub fn machines(&self) -> &Loadable<Vec<String>> {
        &self.machines
    }

    pub fn retry_metadata(&mut self, kind: MetadataKind) -> AppCommand {
        match kind {
            MetadataKind::Projects => self.projects = Loadable::Loading,
            MetadataKind::Agents => self.agents = Loadable::Loading,
            MetadataKind::Machines => self.machines = Loadable::Loading,
        }
        AppCommand::FetchMetadata(kind)
    }

    pub fn timeline_cursor(&self) -> usize {
        self.timeline_cursor
    }

    pub fn timeline_inspection_active(&self) -> bool {
        self.timeline_inspection_active
    }

    pub(crate) fn inspected_bucket(&self) -> Option<&crate::wire::Bucket> {
        self.timeline_inspection_active
            .then(|| self.report()?.buckets.get(self.timeline_cursor))
            .flatten()
    }

    pub fn toggle_timeline_inspection(&mut self) {
        let selected_session_id = self.selected_session_id();
        if self.timeline_inspection_active {
            self.timeline_inspection_active = false;
        } else if self
            .report()
            .is_some_and(|report| !report.buckets.is_empty())
        {
            self.timeline_inspection_active = true;
        }
        self.restore_session_selection_or_first(selected_session_id.as_deref());
    }

    pub fn move_timeline(&mut self, delta: isize) {
        let selected_session_id = self.selected_session_id();
        if let Some(report) = self.report() {
            let last = report.buckets.len().saturating_sub(1);
            self.timeline_cursor = self.timeline_cursor.saturating_add_signed(delta).min(last);
        } else {
            self.timeline_cursor = 0;
        }
        self.restore_session_selection_or_first(selected_session_id.as_deref());
    }

    pub fn breakdown_category(&self) -> BreakdownCategory {
        self.breakdown_category
    }

    pub fn set_breakdown_category(&mut self, category: BreakdownCategory) {
        self.breakdown_category = category;
    }

    pub fn breakdown_value(&self) -> BreakdownValue {
        self.breakdown_value
    }

    pub fn toggle_breakdown_value(&mut self) {
        self.breakdown_value = match self.breakdown_value {
            BreakdownValue::AgentMinutes => BreakdownValue::Cost,
            BreakdownValue::Cost => BreakdownValue::AgentMinutes,
        };
    }

    pub fn breakdown_rows(&self) -> &[KeyMinutes] {
        let Some(report) = self.report() else {
            return &[];
        };
        match self.breakdown_category {
            BreakdownCategory::Project => &report.by_project,
            BreakdownCategory::Model => &report.by_model,
            BreakdownCategory::Agent => &report.by_agent,
        }
    }

    pub fn compact_region(&self) -> CompactRegion {
        self.compact_region
    }

    pub fn set_color_mode(&mut self, color_mode: ColorMode) {
        self.color_mode = color_mode;
    }

    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    fn next_generation(&mut self) -> u64 {
        self.generation_counter = self
            .generation_counter
            .checked_add(1)
            .expect("Activity request generation exhausted");
        self.generation_counter
    }

    fn pending_generation(&self) -> Option<u64> {
        match &self.report_state {
            ReportState::InitialLoading { generation } => *generation,
            ReportState::Refreshing { generation, .. } => Some(*generation),
            ReportState::Ready { .. } | ReportState::Stale { .. } | ReportState::Failed(_) => None,
        }
    }

    pub(crate) fn has_in_flight_report(&self) -> bool {
        self.pending_generation().is_some()
    }

    pub(crate) fn move_breakdown(&mut self, delta: isize) {
        const CATEGORIES: [BreakdownCategory; 3] = [
            BreakdownCategory::Project,
            BreakdownCategory::Model,
            BreakdownCategory::Agent,
        ];
        let current = CATEGORIES
            .iter()
            .position(|category| *category == self.breakdown_category)
            .expect("closed breakdown category");
        let next = (current as isize + delta).rem_euclid(CATEGORIES.len() as isize) as usize;
        self.breakdown_category = CATEGORIES[next];
    }
}

impl<T> From<Result<T, ApiError>> for Loadable<T> {
    fn from(result: Result<T, ApiError>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(error) => Self::Failed(error),
        }
    }
}
