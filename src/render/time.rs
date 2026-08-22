// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, FixedOffset, LocalResult, Offset, TimeZone, Utc};
use chrono_tz::Tz;

pub(super) fn format_clock(value: DateTime<FixedOffset>, timezone: Tz) -> String {
    let local = value.with_timezone(&timezone);
    let format = if is_ambiguous(&local) {
        "%H:%M %Z"
    } else {
        "%H:%M"
    };
    local.format(format).to_string()
}

pub(super) fn format_interval(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    timezone: Tz,
) -> String {
    format_interval_with_separator(start, end, timezone, "-")
}

pub(super) fn format_window(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    timezone: Tz,
) -> String {
    format_interval_with_separator(start, end, timezone, "–")
}

fn format_interval_with_separator(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    timezone: Tz,
    separator: &str,
) -> String {
    let start = start.with_timezone(&timezone);
    let end = end.with_timezone(&timezone);
    if start.offset().fix() == end.offset().fix() && !is_ambiguous(&start) && !is_ambiguous(&end) {
        return format!(
            "{}{separator}{}",
            start.format("%H:%M"),
            end.format("%H:%M")
        );
    }
    format!(
        "{} {}{separator}{} {}",
        start.format("%H:%M"),
        start.format("%Z"),
        end.format("%H:%M"),
        end.format("%Z")
    )
}

pub(super) fn format_relative(now: DateTime<Utc>, timestamp: &str) -> String {
    let Ok(parsed) = DateTime::parse_from_rfc3339(timestamp) else {
        return "—".to_owned();
    };
    let then = parsed.with_timezone(&Utc);
    let seconds = now.signed_duration_since(then).num_seconds();
    if seconds < 60 {
        return "just now".to_owned();
    }
    let seconds = seconds as u64;
    if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else if seconds < 604_800 {
        format!("{}d ago", seconds / 86_400)
    } else {
        then.format("%b %d").to_string()
    }
}

fn is_ambiguous(local: &DateTime<Tz>) -> bool {
    matches!(
        local.timezone().from_local_datetime(&local.naive_local()),
        LocalResult::Ambiguous(_, _)
    )
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::format_relative;

    #[test]
    fn relative_time_uses_ended_at_scale_and_placeholder_for_junk() {
        let now = Utc.with_ymd_and_hms(2026, 8, 8, 17, 21, 12).unwrap();
        assert_eq!(format_relative(now, "2026-08-08T17:21:00Z"), "just now");
        assert_eq!(format_relative(now, "2026-08-08T16:47:00Z"), "34m ago");
        assert_eq!(format_relative(now, "2026-08-08T13:21:12Z"), "4h ago");
        assert_eq!(format_relative(now, "2026-08-05T17:21:12Z"), "3d ago");
        assert_eq!(format_relative(now, "not-a-timestamp"), "—");
    }
}
