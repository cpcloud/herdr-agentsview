// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::ffi::OsStr;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::Utc;
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
use crate::app::{App, AppCommand, InputKey, MetadataKind, ReportState};
use crate::config::PluginConfig;
use crate::render::{self, TerminalCapabilities};
use crate::wire::{AgentInfo, ProjectInfo, Report, ReportSelection};

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
    Projects {
        token: u64,
        result: Result<Vec<ProjectInfo>, ApiError>,
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
    projects: Option<OwnedTask>,
    agents: Option<OwnedTask>,
    machines: Option<OwnedTask>,
    task_token: u64,
    report_timeout_configured: bool,
    report_wait_intervals: u32,
    scheduled_supersessions: u32,
    refresh_interval: Duration,
    next_refresh: Instant,
    executor: tokio::runtime::Handle,
}

impl Runtime {
    pub fn new(config: &PluginConfig) -> anyhow::Result<Self> {
        Self::new_at(config, Instant::now())
    }

    #[doc(hidden)]
    pub fn new_at(config: &PluginConfig, now: Instant) -> anyhow::Result<Self> {
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
            projects: None,
            agents: None,
            machines: None,
            task_token: 0,
            report_timeout_configured: config.request_timeout.is_some(),
            report_wait_intervals: 0,
            scheduled_supersessions: 0,
            refresh_interval: config.refresh_interval,
            next_refresh,
            executor,
        })
    }

    pub fn start(&mut self, app: &mut App) {
        self.dispatch(AppCommand::FetchReport(app.begin_foreground_load()));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Projects));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Agents));
        self.dispatch(AppCommand::FetchMetadata(MetadataKind::Machines));
    }

    pub fn dispatch(&mut self, command: AppCommand) -> bool {
        match command {
            AppCommand::FetchReport(request) => {
                self.reset_report_backoff();
                self.spawn_report(request);
            }
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
                app.apply_report(result, received_at);
            }
            Completion::Projects { token, result } => {
                if !take_current(&mut self.projects, token) {
                    return false;
                }
                app.apply_projects(result);
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
            MetadataKind::Agents => &mut self.agents,
            MetadataKind::Machines => &mut self.machines,
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
        abort_task(&mut self.projects);
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
    let mut runtime = Runtime::new(&config)?;
    runtime.start(&mut app);
    run_terminal(&mut app, &mut runtime, config.timezone)
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

    use crate::api::ApiError;
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
            serde_json::from_str(include_str!("../tests/fixtures/report-v5.json")).unwrap();
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
            serde_json::from_str(include_str!("../tests/fixtures/report-v5.json")).unwrap();
        first_report.totals.sessions = 41;
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
            serde_json::from_str(include_str!("../tests/fixtures/report-v5.json")).unwrap();
        replacement_report.totals.sessions = 42;
        runtime
            .sender
            .send(Completion::Report {
                token: replacement_token,
                result: Ok(Box::new(replacement_report)),
                received_at: "2026-08-08T17:22:00Z".parse().unwrap(),
            })
            .unwrap();

        assert_eq!(runtime.drain_events(&mut app), 1);
        assert_eq!(app.report().unwrap().totals.sessions, 42);
    }
}
