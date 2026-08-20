// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsStr;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Utc};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::available_color_count;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::{ActivityClient, ApiError};
use crate::app::{App, AppCommand, InputKey, MetadataKind, ReportState, SessionPageRequest};
use crate::config::PluginConfig;
use crate::render::{self, TerminalCapabilities};
use crate::wire::{AgentInfo, BranchInfo, ProjectInfo, ProjectResolution, Report, ReportSelection};

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STATUS_REDRAW_INTERVAL: Duration = Duration::from_secs(1);

struct OwnedTask {
    token: u64,
    handle: JoinHandle<()>,
}

enum Completion {
    Report {
        token: u64,
        result: Result<Box<Report>, ApiError>,
        received_at: chrono::DateTime<Utc>,
    },
    SessionPage {
        token: u64,
        request: SessionPageRequest,
        result: Result<crate::api::SessionFetch, ApiError>,
        received_at: chrono::DateTime<Utc>,
    },
    Projects {
        token: u64,
        result: Result<Vec<ProjectInfo>, ApiError>,
    },
    Branches {
        token: u64,
        result: Result<Vec<BranchInfo>, ApiError>,
    },
    Agents {
        token: u64,
        result: Result<Vec<AgentInfo>, ApiError>,
    },
    Machines {
        token: u64,
        result: Result<Vec<String>, ApiError>,
    },
}

pub struct Runtime {
    client: ActivityClient,
    sender: mpsc::UnboundedSender<Completion>,
    receiver: mpsc::UnboundedReceiver<Completion>,
    report: Option<OwnedTask>,
    session_page: Option<OwnedTask>,
    projects: Option<OwnedTask>,
    branches: Option<OwnedTask>,
    agents: Option<OwnedTask>,
    machines: Option<OwnedTask>,
    task_token: u64,
    report_timeout_configured: bool,
    report_wait_intervals: u32,
    scheduled_supersessions: u32,
    refresh_interval: Duration,
    next_refresh: Instant,
    executor: tokio::runtime::Handle,
    source_scope: Option<PendingSourceScope>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceContext {
    normalized_remote: String,
    branch: String,
}

impl SourceContext {
    pub fn new(normalized_remote: impl Into<String>, branch: impl Into<String>) -> Self {
        Self {
            normalized_remote: normalized_remote.into(),
            branch: branch.into(),
        }
    }

    fn from_env() -> Option<Self> {
        let normalized_remote = std::env::var(crate::herdr::SOURCE_REMOTE_ENV).ok()?;
        let branch = std::env::var(crate::herdr::SOURCE_BRANCH_ENV).ok()?;
        if normalized_remote.is_empty() || branch.is_empty() {
            return None;
        }
        Some(Self::new(normalized_remote, branch))
    }
}

struct PendingSourceScope {
    context: SourceContext,
    report: Option<SourceReport>,
    branches: Option<Result<Vec<BranchInfo>, ApiError>>,
}

type SourceReport = (Result<Box<Report>, ApiError>, DateTime<Utc>);

impl Runtime {
    pub fn new(config: &PluginConfig) -> anyhow::Result<Self> {
        Self::new_at(config, Instant::now())
    }

    pub fn new_with_source_context(
        config: &PluginConfig,
        source_context: Option<SourceContext>,
    ) -> anyhow::Result<Self> {
        Self::new_at_with_source_context(config, Instant::now(), source_context)
    }

    #[doc(hidden)]
    pub fn new_at(config: &PluginConfig, now: Instant) -> anyhow::Result<Self> {
        Self::new_at_with_source_context(config, now, None)
    }

    fn new_at_with_source_context(
        config: &PluginConfig,
        now: Instant,
        source_context: Option<SourceContext>,
    ) -> anyhow::Result<Self> {
        let executor = tokio::runtime::Handle::try_current()
            .context("Activity request runtime is not running")?;
        let next_refresh = now
            .checked_add(config.refresh_interval)
            .context("refresh interval is too large for the runtime clock")?;
        let (sender, receiver) = mpsc::unbounded_channel();
        Ok(Self {
            client: ActivityClient::new(config)?,
            sender,
            receiver,
            report: None,
            session_page: None,
            projects: None,
            branches: None,
            agents: None,
            machines: None,
            task_token: 0,
            report_timeout_configured: config.request_timeout.is_some(),
            report_wait_intervals: 0,
            scheduled_supersessions: 0,
            refresh_interval: config.refresh_interval,
            next_refresh,
            executor,
            source_scope: source_context.map(|context| PendingSourceScope {
                context,
                report: None,
                branches: None,
            }),
        })
    }

    pub fn start(&mut self, app: &mut App) {
        if self.source_scope.is_some() {
            self.spawn_report(app.begin_foreground_load());
        } else {
            self.dispatch(AppCommand::FetchReport(app.begin_foreground_load()));
        }
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Projects));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Branches));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Agents));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Machines));
    }

    pub fn dispatch(&mut self, command: AppCommand) -> bool {
        match command {
            AppCommand::FetchReport(request) => {
                self.source_scope = None;
                self.reset_report_backoff();
                self.spawn_report(request);
            }
            AppCommand::FetchSessionPage(request) => self.spawn_session_page(request),
            AppCommand::CancelSessionPage => abort_task(&mut self.session_page),
            AppCommand::FetchMetadata(kind) => self.spawn_metadata(kind),
            AppCommand::Quit => return true,
        }
        false
    }

    pub fn tick(&mut self, app: &mut App, now: Instant) -> anyhow::Result<()> {
        if now < self.next_refresh {
            return Ok(());
        }
        self.next_refresh = now
            .checked_add(self.refresh_interval)
            .context("refresh interval exceeds the runtime clock")?;
        let request = self.scheduled_report_request(app);
        if let Some(request) = request {
            self.spawn_report(request);
        }
        Ok(())
    }

    pub fn drain_events(&mut self, app: &mut App) -> usize {
        let mut applied = 0;
        while let Ok(completion) = self.receiver.try_recv() {
            if self.apply_completion(app, completion) {
                applied += 1;
            }
        }
        applied
    }

    fn spawn_report(&mut self, selection: ReportSelection) {
        abort_task(&mut self.report);
        abort_task(&mut self.session_page);
        let token = self.next_task_token();
        let client = self.client.clone();
        let sender = self.sender.clone();
        let handle = self.executor.spawn(async move {
            let result = client.fetch_report(&selection).await.map(Box::new);
            let _ = sender.send(Completion::Report {
                token,
                result,
                received_at: Utc::now(),
            });
        });
        self.report = Some(OwnedTask { token, handle });
    }

    fn spawn_session_page(&mut self, request: SessionPageRequest) {
        abort_task(&mut self.session_page);
        let token = self.next_task_token();
        let client = self.client.clone();
        let sender = self.sender.clone();
        let task_request = request.clone();
        let handle = self.executor.spawn(async move {
            let result = client
                .fetch_bucket_sessions(&task_request.report_id, task_request.bucket)
                .await;
            let _ = sender.send(Completion::SessionPage {
                token,
                request: task_request,
                result,
                received_at: Utc::now(),
            });
        });
        self.session_page = Some(OwnedTask { token, handle });
    }

    fn spawn_metadata(&mut self, kind: MetadataKind) {
        let token = self.next_task_token();
        let slot = self.metadata_slot_mut(kind);
        abort_task(slot);
        let client = self.client.clone();
        let sender = self.sender.clone();
        let handle = self.executor.spawn(async move {
            let completion = match kind {
                MetadataKind::Projects => Completion::Projects {
                    token,
                    result: client.fetch_projects().await,
                },
                MetadataKind::Branches => Completion::Branches {
                    token,
                    result: client.fetch_branches().await,
                },
                MetadataKind::Agents => Completion::Agents {
                    token,
                    result: client.fetch_agents().await,
                },
                MetadataKind::Machines => Completion::Machines {
                    token,
                    result: client.fetch_machines().await,
                },
            };
            let _ = sender.send(completion);
        });
        *self.metadata_slot_mut(kind) = Some(OwnedTask { token, handle });
    }

    fn apply_completion(&mut self, app: &mut App, completion: Completion) -> bool {
        match completion {
            Completion::Report {
                token,
                result,
                received_at,
            } => {
                if !take_current(&mut self.report, token) {
                    return false;
                }
                self.reset_report_backoff();
                if let Some(pending) = &mut self.source_scope {
                    pending.report = Some((result, received_at));
                    self.finish_source_scope(app);
                } else {
                    self.apply_report_result(app, result, received_at);
                }
            }
            Completion::SessionPage {
                token,
                request,
                result,
                received_at,
            } => {
                if !take_current(&mut self.session_page, token) {
                    return false;
                }
                app.apply_session_page(&request, result, received_at);
            }
            Completion::Projects { token, result } => {
                if !take_current(&mut self.projects, token) {
                    return false;
                }
                app.apply_projects(result);
            }
            Completion::Branches { token, result } => {
                if !take_current(&mut self.branches, token) {
                    return false;
                }
                app.apply_branches(result.clone());
                if let Some(pending) = &mut self.source_scope {
                    pending.branches = Some(result);
                    self.finish_source_scope(app);
                }
            }
            Completion::Agents { token, result } => {
                if !take_current(&mut self.agents, token) {
                    return false;
                }
                app.apply_agents(result);
            }
            Completion::Machines { token, result } => {
                if !take_current(&mut self.machines, token) {
                    return false;
                }
                app.apply_machines(result);
            }
        }
        true
    }

    fn metadata_slot_mut(&mut self, kind: MetadataKind) -> &mut Option<OwnedTask> {
        match kind {
            MetadataKind::Projects => &mut self.projects,
            MetadataKind::Branches => &mut self.branches,
            MetadataKind::Agents => &mut self.agents,
            MetadataKind::Machines => &mut self.machines,
        }
    }

    fn finish_source_scope(&mut self, app: &mut App) {
        let ready = self
            .source_scope
            .as_ref()
            .is_some_and(|pending| pending.report.is_some() && pending.branches.is_some());
        if !ready {
            return;
        }
        let mut pending = self.source_scope.take().expect("checked source scope");
        let (report, received_at) = pending.report.take().expect("checked source report");
        let branches = pending.branches.take().expect("checked source branches");
        let (report, branches) = match (report, branches) {
            (Ok(report), Ok(branches)) => (report, branches),
            (result, _) => {
                self.apply_report_result(app, result, received_at);
                return;
            }
        };
        let Some((project, token)) = resolve_source_scope(&report, &branches, &pending.context)
        else {
            self.apply_report_result(app, Ok(report), received_at);
            return;
        };
        app.apply_source_scope(project, token);
        self.spawn_report(app.begin_foreground_load());
    }

    fn apply_report_result(
        &mut self,
        app: &mut App,
        result: Result<Box<Report>, ApiError>,
        received_at: DateTime<Utc>,
    ) {
        app.apply_report(result, received_at);
        if let Some(request) = app.begin_session_page_request() {
            self.spawn_session_page(request);
        }
    }

    fn scheduled_report_request(&mut self, app: &mut App) -> Option<ReportSelection> {
        if self.report.is_none() {
            self.reset_report_backoff();
            return app.begin_scheduled_load();
        }
        if self.report_timeout_configured {
            return None;
        }
        self.report_wait_intervals = self.report_wait_intervals.saturating_add(1);
        let required_intervals = 1_u32
            .checked_shl(self.scheduled_supersessions)
            .unwrap_or(u32::MAX);
        if self.report_wait_intervals < required_intervals {
            return None;
        }
        let request = app.supersede_pending_load()?;
        self.report_wait_intervals = 0;
        self.scheduled_supersessions = self.scheduled_supersessions.saturating_add(1);
        Some(request)
    }

    fn reset_report_backoff(&mut self) {
        self.report_wait_intervals = 0;
        self.scheduled_supersessions = 0;
    }

    fn next_task_token(&mut self) -> u64 {
        self.task_token = self
            .task_token
            .checked_add(1)
            .expect("Activity task token exhausted");
        self.task_token
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        abort_task(&mut self.report);
        abort_task(&mut self.session_page);
        abort_task(&mut self.projects);
        abort_task(&mut self.branches);
        abort_task(&mut self.agents);
        abort_task(&mut self.machines);
    }
}

fn abort_task(task: &mut Option<OwnedTask>) {
    if let Some(task) = task.take() {
        task.handle.abort();
    }
}

fn take_current(task: &mut Option<OwnedTask>, token: u64) -> bool {
    if task.as_ref().map(|task| task.token) != Some(token) {
        return false;
    }
    *task = None;
    true
}

pub fn run(config: PluginConfig) -> anyhow::Result<()> {
    let executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build Activity request runtime")?;
    let _runtime_context = executor.enter();
    let today = Utc::now().with_timezone(&config.timezone).date_naive();
    let mut app = App::new(
        ReportSelection::new(today, config.timezone),
        config.refresh_interval,
    );
    app.set_color_mode(terminal_capabilities().color_mode());
    let mut runtime = Runtime::new_with_source_context(&config, SourceContext::from_env())?;
    runtime.start(&mut app);
    run_terminal(&mut app, &mut runtime, config.timezone)
}

fn resolve_source_scope(
    report: &Report,
    branches: &[BranchInfo],
    context: &SourceContext,
) -> Option<(String, String)> {
    let mut projects = report.projects.values().filter(|project| {
        project.resolution == ProjectResolution::Resolved
            && project
                .identity
                .as_ref()
                .and_then(|identity| identity.normalized_remote.as_deref())
                == Some(context.normalized_remote.as_str())
    });
    let project = projects.next()?;
    if projects.next().is_some() {
        return None;
    }
    let mut matches = branches.iter().filter(|branch| {
        branch.project == project.display_label && branch.branch == context.branch
    });
    let branch = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some((project.display_label.clone(), branch.token.clone()))
}

fn run_terminal(
    app: &mut App,
    runtime: &mut Runtime,
    timezone: chrono_tz::Tz,
) -> anyhow::Result<()> {
    enable_raw_mode().context("enable terminal raw mode")?;
    let mut restore = RestoreTerminal {
        raw_mode: true,
        alternate_screen: false,
        cursor_hidden: false,
    };
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter terminal alternate screen")?;
    restore.alternate_screen = true;
    execute!(stdout, Hide).context("hide terminal cursor")?;
    restore.cursor_hidden = true;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("initialize terminal")?;
    let loop_result = run_loop(&mut terminal, app, runtime, timezone);
    let cursor_result = terminal.show_cursor().context("restore terminal cursor");
    if cursor_result.is_ok() {
        restore.cursor_hidden = false;
    }
    drop(terminal);
    let restore_result = restore.restore().context("restore terminal mode");
    loop_result.and(cursor_result).and(restore_result)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    runtime: &mut Runtime,
    timezone: chrono_tz::Tz,
) -> anyhow::Result<()> {
    let mut redraw_requested = true;
    let mut next_status_redraw = Instant::now();
    loop {
        redraw_requested |= runtime.drain_events(app) > 0;
        let now = Instant::now();
        runtime.tick(app, now)?;
        if redraw_required(app, redraw_requested, now >= next_status_redraw) {
            terminal
                .draw(|frame| {
                    let plan = synchronize_layout(app, frame.area());
                    render::draw(frame, app, &plan);
                })
                .context("draw Activity dashboard")?;
            redraw_requested = false;
            next_status_redraw = now + STATUS_REDRAW_INTERVAL;
        }

        if !event::poll(INPUT_POLL_INTERVAL).context("poll terminal input")? {
            continue;
        }
        match event::read().context("read terminal input")? {
            Event::Key(key) if accepts_key_event(key) => {
                let today = Utc::now().with_timezone(&timezone).date_naive();
                if let Some(input) = map_key(key) {
                    if let Some(command) = app.handle_input(input, today) {
                        if runtime.dispatch(command) {
                            return Ok(());
                        }
                    }
                    redraw_requested = true;
                }
            }
            Event::Resize(_, _) => redraw_requested = true,
            _ => {}
        }
    }
}

fn redraw_required(app: &App, state_changed: bool, clock_due: bool) -> bool {
    state_changed
        || matches!(
            app.report_state(),
            ReportState::InitialLoading | ReportState::Refreshing { .. }
        )
        || clock_due
            && matches!(
                app.report_state(),
                ReportState::Ready { .. } | ReportState::Stale { .. }
            )
}

fn synchronize_layout(app: &mut App, area: Rect) -> render::FramePlan {
    let plan = render::FramePlan::new(app, area);
    if let Some(visible_rows) = plan.session_viewport_rows() {
        app.set_session_viewport_rows(visible_rows);
    }
    plan
}

fn accepts_key_event(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn map_key(key: KeyEvent) -> Option<InputKey> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(InputKey::Quit);
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    match key.code {
        KeyCode::Tab => Some(InputKey::Tab),
        KeyCode::BackTab => Some(InputKey::BackTab),
        KeyCode::Left => Some(InputKey::Left),
        KeyCode::Right => Some(InputKey::Right),
        KeyCode::Up => Some(InputKey::Up),
        KeyCode::Down => Some(InputKey::Down),
        KeyCode::Enter => Some(InputKey::Enter),
        KeyCode::Esc => Some(InputKey::Escape),
        KeyCode::Backspace => Some(InputKey::Backspace),
        KeyCode::Char(character) => Some(InputKey::Char(character)),
        _ => None,
    }
}

fn terminal_capabilities() -> TerminalCapabilities {
    TerminalCapabilities {
        color_count: available_color_count(),
        no_color: no_color_requested(std::env::var_os("NO_COLOR").as_deref()),
        term_is_dumb: std::env::var("TERM").is_ok_and(|term| term == "dumb"),
    }
}

fn no_color_requested(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

struct RestoreTerminal {
    raw_mode: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
}

impl RestoreTerminal {
    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.cursor_hidden {
            if let Err(error) = execute!(io::stdout(), Show) {
                first_error = Some(error);
            }
            self.cursor_hidden = false;
        }
        if self.alternate_screen {
            if let Err(error) = execute!(io::stdout(), LeaveAlternateScreen) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            self.alternate_screen = false;
        }
        if self.raw_mode {
            if let Err(error) = disable_raw_mode() {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            self.raw_mode = false;
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::time::Duration;

    use chrono::NaiveDate;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use crate::api::{ApiError, SessionFetch};
    use crate::app::{App, AppCommand, InputKey};
    use crate::config::PluginConfig;
    use crate::wire::{Report, ReportSelection};

    use super::{
        map_key, no_color_requested, redraw_required, synchronize_layout, Completion, OwnedTask,
        Runtime,
    };

    #[test]
    fn empty_no_color_value_keeps_terminal_colors_enabled() {
        // If an empty value disables color, inherited placeholder variables silently erase
        // categorical distinctions even though the NO_COLOR convention excludes empty strings.
        assert!(!no_color_requested(None));
        assert!(!no_color_requested(Some(OsStr::new(""))));
        assert!(no_color_requested(Some(OsStr::new("1"))));
    }

    #[test]
    fn ready_dashboard_uses_a_clock_deadline_instead_of_every_input_poll() {
        // If an unchanged ready dashboard redraws on every input poll, large day reports are
        // repeatedly sorted; if it never redraws, the wall-clock freshness label freezes.
        let app = ready_app();

        assert!(!redraw_required(&app, false, false));
        assert!(redraw_required(&app, true, false));
        assert!(redraw_required(&app, false, true));
    }

    #[test]
    fn loading_dashboard_keeps_requesting_spinner_frames() {
        // If idle-frame suppression also stops loading frames, the one visible Braille
        // spinner freezes while an Activity request is in flight.
        let selection = ReportSelection::new(
            NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            "America/New_York".parse().unwrap(),
        );
        let mut app = App::new(selection, Duration::from_secs(300));
        app.begin_foreground_load();

        assert!(redraw_required(&app, false, false));
    }

    #[test]
    fn stale_dashboard_redraws_when_its_age_clock_is_due() {
        // If stale data does not share the freshness deadline, its age freezes even though
        // the warning remains mounted until a successful retry.
        let mut app = ready_app();
        app.begin_refresh().unwrap();
        app.apply_report(
            Err(ApiError::timeout()),
            "2026-08-08T17:21:01Z".parse().unwrap(),
        );

        assert!(redraw_required(&app, false, true));
    }

    #[test]
    fn layout_synchronization_keeps_session_scroll_inside_the_rendered_viewport() {
        // If TUI orchestration never publishes its viewport to the app, keyboard scrolling
        // keeps using the previous pane height and can leave the selected row off-screen.
        let mut app = ready_app();
        app.move_session(2, 1);
        assert_eq!(app.session_scroll(), 2);

        synchronize_layout(&mut app, Rect::new(0, 0, 200, 50));

        assert_eq!(app.session_scroll(), 0);
    }

    fn ready_app() -> App {
        let selection = ReportSelection::new(
            NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            "America/New_York".parse().unwrap(),
        );
        let mut app = App::new(selection, Duration::from_secs(300));
        app.begin_foreground_load();
        let report: Report =
            serde_json::from_str(include_str!("../tests/fixtures/report-v6.json")).unwrap();
        app.apply_report(
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        app
    }

    #[test]
    fn modified_character_keys_do_not_trigger_bare_dashboard_actions() {
        // If modifiers are discarded, common terminal chords silently refresh, switch regions,
        // or close the pane as though the user pressed the bare character.
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT)),
            None
        );
    }

    #[test]
    fn control_c_remains_a_conventional_terminal_exit() {
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(InputKey::Quit)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_report_completion_is_ignored_after_replacement_dispatch() {
        // If replacement dispatch only aborts the old task without invalidating an already
        // queued completion, the old report ends loading and causes the new result to be ignored.
        let config = PluginConfig {
            api_base_url: "http://127.0.0.1:9/".parse().unwrap(),
            request_timeout: Some(Duration::from_secs(2)),
            refresh_interval: Duration::from_secs(300),
            timezone: "America/New_York".parse().unwrap(),
            auth: None,
        };
        let selection = ReportSelection::new(
            NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(),
            config.timezone,
        );
        let mut app = App::new(selection, config.refresh_interval);
        app.begin_foreground_load();
        let mut runtime = Runtime::new(&config).unwrap();
        let first_token = runtime.next_task_token();
        runtime.report = Some(OwnedTask {
            token: first_token,
            handle: tokio::spawn(std::future::pending()),
        });
        let mut first_report: Report =
            serde_json::from_str(include_str!("../tests/fixtures/report-v6.json")).unwrap();
        first_report.totals.output_tokens = 41;
        runtime
            .sender
            .send(Completion::Report {
                token: first_token,
                result: Ok(Box::new(first_report)),
                received_at: "2026-08-08T17:21:00Z".parse().unwrap(),
            })
            .unwrap();
        assert!(!runtime.receiver.is_empty());

        app.set_project(Some("project-beta".to_owned()));
        let replacement = app.begin_foreground_load();
        assert!(!runtime.dispatch(AppCommand::FetchReport(replacement)));
        let replacement_token = runtime.report.as_ref().unwrap().token;
        runtime.report.as_ref().unwrap().handle.abort();

        assert_eq!(runtime.drain_events(&mut app), 0);
        assert!(app.report().is_none());

        let mut replacement_report: Report =
            serde_json::from_str(include_str!("../tests/fixtures/report-v6.json")).unwrap();
        replacement_report.totals.output_tokens = 42;
        runtime
            .sender
            .send(Completion::Report {
                token: replacement_token,
                result: Ok(Box::new(replacement_report)),
                received_at: "2026-08-08T17:22:00Z".parse().unwrap(),
            })
            .unwrap();

        assert_eq!(runtime.drain_events(&mut app), 1);
        assert_eq!(app.report().unwrap().totals.output_tokens, 42);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_session_page_completion_is_ignored_after_same_bucket_replacement() {
        // If the task token gate is removed, a queued old page can satisfy the same report and
        // bucket identity and overwrite the replacement request before its result arrives.
        let config = PluginConfig {
            api_base_url: "http://127.0.0.1:9/".parse().unwrap(),
            request_timeout: Some(Duration::from_secs(2)),
            refresh_interval: Duration::from_secs(300),
            timezone: "America/New_York".parse().unwrap(),
            auth: None,
        };
        let mut app = ready_app();
        app.toggle_timeline_inspection();
        let request = app
            .begin_session_page_request()
            .expect("active bucket page request");
        let old_row = app.report().unwrap().by_session[0].clone();
        let replacement_row = app.report().unwrap().by_session[1].clone();
        let mut runtime = Runtime::new(&config).unwrap();
        let first_token = runtime.next_task_token();
        runtime.session_page = Some(OwnedTask {
            token: first_token,
            handle: tokio::spawn(std::future::pending()),
        });
        runtime
            .sender
            .send(Completion::SessionPage {
                token: first_token,
                request: request.clone(),
                result: Ok(SessionFetch::Rows(vec![old_row])),
                received_at: "2026-08-08T17:21:00Z".parse().unwrap(),
            })
            .unwrap();
        assert!(!runtime.receiver.is_empty());

        assert!(!runtime.dispatch(AppCommand::FetchSessionPage(request.clone())));
        let replacement_token = runtime.session_page.as_ref().unwrap().token;
        runtime.session_page.as_ref().unwrap().handle.abort();

        assert_eq!(runtime.drain_events(&mut app), 0);
        assert!(app.session_page_request().is_some());

        runtime
            .sender
            .send(Completion::SessionPage {
                token: replacement_token,
                request,
                result: Ok(SessionFetch::Rows(vec![replacement_row])),
                received_at: "2026-08-08T17:22:00Z".parse().unwrap(),
            })
            .unwrap();

        assert_eq!(runtime.drain_events(&mut app), 1);
        assert_eq!(app.displayed_sessions()[0].session_id, "session-beta");
    }
}
