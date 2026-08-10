use chrono::{DateTime, FixedOffset, LocalResult, Offset, TimeZone};
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

fn is_ambiguous(local: &DateTime<Tz>) -> bool {
    matches!(
        local.timezone().from_local_datetime(&local.naive_local()),
        LocalResult::Ambiguous(_, _)
    )
}
