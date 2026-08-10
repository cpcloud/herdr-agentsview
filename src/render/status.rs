use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::api::ApiErrorKind;
use crate::app::{App, ReportState};

use super::style::{clip_with_ellipsis, Palette};

pub(super) const RECOVERY_HINT: &str = " · r retry";

pub(super) fn render_report_notice(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    palette: Palette,
) -> bool {
    let Some(notice) = report_notice(app) else {
        return false;
    };
    let style = if matches!(app.report_state(), ReportState::Failed(_)) {
        palette.error()
    } else {
        palette.muted()
    };
    Paragraph::new(Line::from(Span::styled(
        clip_with_ellipsis(&notice, usize::from(area.width)),
        style,
    )))
    .render(area, buffer);
    true
}

pub(super) fn header_status(app: &App, now: DateTime<Utc>) -> String {
    match app.report_state() {
        ReportState::InitialLoading { .. } => String::new(),
        ReportState::Ready { received_at, .. } => format!("Updated {} ago", age(now, *received_at)),
        ReportState::Refreshing { received_at, .. } => {
            format!("Last update {} ago", age(now, *received_at))
        }
        ReportState::Stale {
            received_at, error, ..
        } => with_recovery_hint(format!(
            "Stale {} · {}",
            age(now, *received_at),
            concise_error(&error.message)
        )),
        ReportState::Failed(_) => with_recovery_hint("Activity unavailable"),
    }
}

pub(super) fn header_spinner(app: &App, now: DateTime<Utc>) -> Option<&'static str> {
    matches!(
        app.report_state(),
        ReportState::InitialLoading { .. } | ReportState::Refreshing { .. }
    )
    .then(|| braille_spinner(now))
}

pub(super) fn report_notice(app: &App) -> Option<String> {
    match app.report_state() {
        ReportState::InitialLoading { .. } => None,
        ReportState::Failed(error) => Some(match error.kind {
            ApiErrorKind::Authentication if error.message.contains("rejected") => {
                with_recovery_hint("Credential rejected · replace the runtime token")
            }
            ApiErrorKind::Authentication => {
                with_recovery_hint("Authentication required · configure a runtime token")
            }
            ApiErrorKind::Forbidden => {
                with_recovery_hint("Access forbidden · check AgentsView authorization")
            }
            ApiErrorKind::Timeout => with_recovery_hint(concise_error(&error.message)),
            ApiErrorKind::Network => {
                with_recovery_hint("Cannot reach AgentsView · check URL and network")
            }
            ApiErrorKind::Protocol => {
                with_recovery_hint(format!("Protocol mismatch · {}", error.message))
            }
            ApiErrorKind::Server => {
                with_recovery_hint(format!("AgentsView server error · {}", error.message))
            }
        }),
        ReportState::Ready { .. } | ReportState::Refreshing { .. } | ReportState::Stale { .. } => {
            app.is_empty().then(|| {
                format!(
                    "No activity for {} · move date or clear filters",
                    app.selection().date
                )
            })
        }
    }
}

fn braille_spinner(now: DateTime<Utc>) -> &'static str {
    const FRAMES: [&str; 10] = ["⠖", "⠲", "⢲", "⢰", "⣰", "⣠", "⣄", "⣆", "⡆", "⡖"];
    let frame = now.timestamp_millis().div_euclid(100) as usize % FRAMES.len();
    FRAMES[frame]
}

fn with_recovery_hint(message: impl AsRef<str>) -> String {
    format!("{}{RECOVERY_HINT}", message.as_ref())
}

fn age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(then).num_seconds().max(0) as u64;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        _ => {
            let hours = seconds / 3_600;
            let minutes = seconds % 3_600 / 60;
            if minutes == 0 {
                format!("{hours}h")
            } else {
                format!("{hours}h{minutes:02}")
            }
        }
    }
}

fn concise_error(message: &str) -> String {
    message
        .split(';')
        .next()
        .unwrap_or(message)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::braille_spinner;

    #[test]
    fn spinner_frames_leave_the_top_braille_row_empty() {
        // If any frame uses dots 1 or 4, the spinner rises above the title's letter body in
        // terminal fonts whose braille cells occupy the full line height.
        for frame in 0..10 {
            let now = Utc.timestamp_millis_opt(frame * 100).unwrap();
            let pattern = braille_spinner(now).chars().next().unwrap() as u32 - 0x2800;
            assert_eq!(pattern & 0b0000_1001, 0, "frame {frame}");
        }
    }
}
