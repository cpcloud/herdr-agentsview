// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ColorMode, Focus};
use crate::wire::Report;

use super::layout::LayoutClass;
use super::style::{clip_with_ellipsis, Palette};
use super::time::{format_clock, format_interval};

pub(super) fn render(
    buffer: &mut Buffer,
    area: Rect,
    app: &App,
    class: LayoutClass,
    palette: Palette,
) {
    let block = palette.block(" Concurrent agents ", app.focus() == Focus::Timeline);
    let inner = block.inner(area);
    block.render(area, buffer);
    let Some(report) = app.report() else {
        return;
    };
    if inner.width == 0 || inner.height == 0 || report.buckets.is_empty() {
        return;
    }

    let inspected_bucket = app.inspected_bucket();
    let legend = match inspected_bucket {
        Some(bucket) if report.bucket_is_future(bucket) => Line::from(vec![
            Span::styled(
                format_interval(bucket.start, bucket.end, report.timezone),
                palette.focus(),
            ),
            Span::styled("  future", palette.muted()),
        ]),
        Some(bucket) => {
            let observed_end = report.observed_bucket_end(bucket);
            Line::from(vec![
                Span::styled(
                    format_interval(bucket.start, observed_end, report.timezone),
                    palette.focus(),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("Interactive {}", bucket.interactive_at_peak),
                    palette.interactive(),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("Automated {}", bucket.automated_at_peak),
                    palette.automated(),
                ),
            ])
        }
        None => match palette.mode() {
            ColorMode::Color => Line::from(vec![
                Span::styled("Interactive", palette.interactive()),
                Span::raw("  "),
                Span::styled("Automated", palette.automated()),
                Span::raw("  · observed zero"),
            ]),
            ColorMode::Monochrome => Line::from(vec![
                Span::styled("I Interactive", palette.interactive()),
                Span::raw("  "),
                Span::styled("A Automated", palette.automated()),
                Span::raw("  · observed zero"),
            ]),
        },
    };
    Paragraph::new(legend).render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);

    let separate_cutoff = report.partial && class != LayoutClass::Compact;
    let reserved = 2 + u16::from(separate_cutoff);
    let chart_height = inner.height.saturating_sub(reserved).max(1);
    let chart = Rect::new(inner.x, inner.y + 1, inner.width, chart_height);
    render_chart(buffer, chart, app, palette);

    let axis_y = chart.y + chart.height;
    if axis_y < inner.y + inner.height {
        let axis = timeline_axis(report, class, usize::from(inner.width));
        Paragraph::new(Line::from(Span::styled(axis, palette.muted())))
            .render(Rect::new(inner.x, axis_y, inner.width, 1), buffer);
    }
    if separate_cutoff {
        let cutoff_y = axis_y + 1;
        if cutoff_y < inner.y + inner.height {
            let as_of = report.as_of.map_or_else(
                || "unknown".to_owned(),
                |value| format_clock(value, report.timezone),
            );
            let label = format!(
                "Observed through {} · as of {as_of} · ┄ future",
                format_clock(report.effective_end, report.timezone)
            );
            Paragraph::new(Line::from(Span::styled(
                clip_with_ellipsis(&label, usize::from(inner.width)),
                palette.muted(),
            )))
            .render(Rect::new(inner.x, cutoff_y, inner.width, 1), buffer);
        }
    }
}

fn timeline_axis(report: &Report, class: LayoutClass, width: usize) -> String {
    if report.partial && class == LayoutClass::Compact {
        let as_of = report.as_of.map_or_else(
            || "?".to_owned(),
            |value| format_clock(value, report.timezone),
        );
        return place_labels(
            &[
                format_clock(report.range_start, report.timezone),
                format!(
                    "obs {} · as-of {as_of} · ┄ future",
                    format_clock(report.effective_end, report.timezone)
                ),
                format_clock(report.range_end, report.timezone),
            ],
            width,
        );
    }

    let tick_count: i64 = if class == LayoutClass::Compact { 3 } else { 5 };
    let duration = report.range_end.signed_duration_since(report.range_start);
    let seconds = duration.num_seconds();
    if seconds <= 0 {
        return clip_with_ellipsis(&format_clock(report.range_start, report.timezone), width);
    }
    let last = tick_count - 1;
    let labels = (0..tick_count)
        .map(|index| {
            let value = report.range_start + chrono::Duration::seconds(seconds * index / last);
            format_clock(value, report.timezone)
        })
        .collect::<Vec<_>>();
    place_labels(&labels, width)
}

fn place_labels(labels: &[String], width: usize) -> String {
    if labels.is_empty() || width == 0 {
        return String::new();
    }
    let mut line = String::new();
    let last = labels.len().saturating_sub(1);
    let mut occupied_until = 0;
    let mut placed_any = false;
    for (index, label) in labels.iter().enumerate() {
        let label = clip_with_ellipsis(label, width);
        let label_width = UnicodeWidthStr::width(label.as_str());
        if label_width == 0 {
            continue;
        }
        let anchor = (index * width.saturating_sub(1))
            .checked_div(last)
            .unwrap_or(0);
        let desired = match index {
            0 => 0,
            _ if index == last => width.saturating_sub(label_width),
            _ => anchor.saturating_sub(label_width / 2),
        };
        let required_start = occupied_until + usize::from(placed_any);
        let start = desired
            .max(required_start)
            .min(width.saturating_sub(label_width));
        if start < required_start || start + label_width > width {
            continue;
        }
        line.push_str(&" ".repeat(start - occupied_until));
        line.push_str(&label);
        occupied_until = start + label_width;
        placed_any = true;
    }
    line.push_str(&" ".repeat(width.saturating_sub(occupied_until)));
    line
}

fn render_chart(buffer: &mut Buffer, area: Rect, app: &App, palette: Palette) {
    let Some(report) = app.report() else {
        return;
    };
    let observed = report.observed_bucket_count();
    let peak = report
        .buckets
        .iter()
        .take(observed)
        .map(|bucket| {
            bucket
                .interactive_at_peak
                .saturating_add(bucket.automated_at_peak)
        })
        .max()
        .unwrap_or(0)
        .max(1);
    let observed_columns = observed_columns(report, usize::from(area.width));
    let cursor_columns = app
        .timeline_inspection_active()
        .then(|| {
            bucket_column_range(
                app.timeline_cursor(),
                report.buckets.len(),
                usize::from(area.width),
            )
        })
        .flatten();
    for x_offset in 0..area.width {
        let bucket = if usize::from(x_offset) < observed_columns {
            column_bucket(
                &report.buckets,
                observed,
                usize::from(x_offset),
                usize::from(area.width),
            )
        } else {
            None
        };
        if let Some(bucket) = bucket {
            let stack = scaled_stack(
                bucket.interactive_at_peak,
                bucket.automated_at_peak,
                peak,
                usize::from(area.height),
            );
            let total_height = stack.interactive + stack.automated + usize::from(stack.mixed);
            for y_offset in 0..area.height {
                let from_bottom = usize::from(area.height - y_offset - 1);
                let cell = &mut buffer[(area.x + x_offset, area.y + y_offset)];
                if total_height == 0 && from_bottom == 0 {
                    cell.set_symbol("·").set_style(palette.muted());
                } else if stack.mixed && from_bottom == 0 {
                    let symbol = if palette.mode() == ColorMode::Color {
                        "▄"
                    } else {
                        "▒"
                    };
                    cell.set_symbol(symbol).set_style(palette.mixed_activity());
                } else if from_bottom < stack.interactive {
                    cell.set_symbol("█").set_style(palette.interactive());
                } else if from_bottom < total_height {
                    let symbol = if palette.mode() == ColorMode::Monochrome {
                        "▓"
                    } else {
                        "█"
                    };
                    cell.set_symbol(symbol).set_style(palette.automated());
                }
            }
        } else {
            let cell = &mut buffer[(area.x + x_offset, area.y + area.height - 1)];
            cell.set_symbol("┄").set_style(palette.muted());
        }
        if cursor_columns
            .as_ref()
            .is_some_and(|columns| columns.contains(&usize::from(x_offset)))
        {
            let cell = &mut buffer[(area.x + x_offset, area.y + area.height - 1)];
            cell.set_style(palette.timeline_cursor(cell.style()));
        }
    }
}

fn bucket_column_range(
    bucket_index: usize,
    bucket_count: usize,
    width: usize,
) -> Option<std::ops::Range<usize>> {
    if bucket_index >= bucket_count || bucket_count == 0 || width == 0 {
        return None;
    }
    let start = bucket_index.saturating_mul(width) / bucket_count;
    let end = (bucket_index + 1)
        .saturating_mul(width)
        .div_ceil(bucket_count)
        .max(start + 1)
        .min(width);
    Some(start.min(width - 1)..end)
}

fn observed_columns(report: &Report, width: usize) -> usize {
    if !report.partial {
        return width;
    }
    let total = report
        .range_end
        .signed_duration_since(report.range_start)
        .num_milliseconds();
    if total <= 0 {
        return 0;
    }
    let observed = report
        .effective_end
        .signed_duration_since(report.range_start)
        .num_milliseconds()
        .clamp(0, total);
    let columns = (observed as u128 * width as u128).div_ceil(total as u128);
    usize::try_from(columns.min(width as u128))
        .expect("observed timeline width is bounded by the terminal width")
}

fn column_bucket(
    buckets: &[crate::wire::Bucket],
    observed: usize,
    column: usize,
    width: usize,
) -> Option<&crate::wire::Bucket> {
    if buckets.is_empty() || width == 0 || column >= width {
        return None;
    }
    let boundary =
        |position: usize| ((position as u128 * buckets.len() as u128) / width as u128) as usize;
    let start = boundary(column);
    let observed = observed.min(buckets.len());
    if start >= observed {
        return None;
    }
    let end = if buckets.len() >= width {
        boundary(column + 1).max(start + 1)
    } else {
        start + 1
    }
    .min(observed);
    buckets[start..end].iter().max_by_key(|bucket| {
        bucket
            .interactive_at_peak
            .saturating_add(bucket.automated_at_peak)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StackHeights {
    interactive: usize,
    automated: usize,
    mixed: bool,
}

fn scaled_stack(interactive: usize, automated: usize, peak: usize, height: usize) -> StackHeights {
    let total = interactive.saturating_add(automated);
    let total_height = scaled_height(total, peak, height);
    if total_height == 0 {
        return StackHeights {
            interactive: 0,
            automated: 0,
            mixed: false,
        };
    }
    match (interactive, automated, total_height) {
        (0, _, _) => StackHeights {
            interactive: 0,
            automated: total_height,
            mixed: false,
        },
        (_, 0, _) => StackHeights {
            interactive: total_height,
            automated: 0,
            mixed: false,
        },
        (_, _, 1) => StackHeights {
            interactive: 0,
            automated: 0,
            mixed: true,
        },
        _ => {
            let rounded_interactive =
                ((interactive as u128 * total_height as u128) + total as u128 / 2) / total as u128;
            let interactive_height = usize::try_from(rounded_interactive)
                .expect("scaled stack height is bounded by the terminal height")
                .clamp(1, total_height - 1);
            StackHeights {
                interactive: interactive_height,
                automated: total_height - interactive_height,
                mixed: false,
            }
        }
    }
}

fn scaled_height(value: usize, peak: usize, height: usize) -> usize {
    if value == 0 || peak == 0 || height == 0 {
        0
    } else {
        let scaled = (value as u128 * height as u128).div_ceil(peak as u128);
        usize::try_from(scaled.min(height as u128))
            .expect("scaled height is bounded by the terminal height")
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::DateTime;
    use chrono::NaiveDate;
    use chrono_tz::UTC;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr;

    use crate::app::{App, ColorMode, Focus};
    use crate::render::layout::LayoutClass;
    use crate::render::style::Palette;
    use crate::wire::{Bucket, Money, Report, ReportSelection};

    use super::{
        bucket_column_range, column_bucket, observed_columns, place_labels, render_chart,
        scaled_stack, timeline_axis,
    };

    #[test]
    fn inspected_bucket_always_owns_visible_chart_columns() {
        // If the chart cursor is derived only from exact bucket-to-column equality, it
        // disappears when sparse data is expanded or dense data is compressed.
        assert_eq!(bucket_column_range(1, 2, 80), Some(40..80));
        assert_eq!(bucket_column_range(17, 288, 80), Some(4..5));
        assert_eq!(bucket_column_range(288, 288, 80), None);
    }

    #[test]
    fn inspected_zero_bucket_marks_only_the_chart_baseline() {
        // If the cursor paints every cell in an empty bucket, the selection looks like an
        // unexplained vertical data bar instead of a time-range highlight.
        let mut report = fixture_report();
        for bucket in &mut report.buckets {
            bucket.interactive_at_peak = 0;
            bucket.automated_at_peak = 0;
        }
        let bucket_count = report.buckets.len();
        let selection = ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(), UTC);
        let mut app = App::new(selection, Duration::from_secs(300));
        app.set_color_mode(ColorMode::Color);
        let request = app.begin_foreground_load();
        app.apply_report(
            request.generation,
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        app.set_focus(Focus::Timeline);
        app.toggle_timeline_inspection();
        let area = Rect::new(0, 0, 24, 3);
        let mut buffer = Buffer::empty(area);

        render_chart(&mut buffer, area, &app, Palette::new(ColorMode::Color));

        let columns = bucket_column_range(0, bucket_count, usize::from(area.width)).unwrap();
        for x in columns {
            assert_eq!(buffer[(x as u16, 0)].bg, Color::Reset);
            assert_eq!(buffer[(x as u16, 1)].bg, Color::Reset);
            assert_eq!(buffer[(x as u16, 2)].bg, Color::DarkGray);
        }
    }

    #[test]
    fn inspected_activity_uses_a_visible_gray_cursor_over_solid_bars() {
        // If selection only changes the background behind a full-block data glyph, the bar
        // hides the cursor completely and the active session slice has no chart marker.
        let report = fixture_report();
        let bucket_count = report.buckets.len();
        let selection = ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(), UTC);
        let mut app = App::new(selection, Duration::from_secs(300));
        app.set_color_mode(ColorMode::Color);
        let request = app.begin_foreground_load();
        app.apply_report(
            request.generation,
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        app.toggle_timeline_inspection();
        let area = Rect::new(0, 0, 24, 3);
        let mut buffer = Buffer::empty(area);

        render_chart(&mut buffer, area, &app, Palette::new(ColorMode::Color));

        let columns = bucket_column_range(0, bucket_count, usize::from(area.width)).unwrap();
        for x in columns {
            let baseline = &buffer[(x as u16, area.height - 1)];
            assert_eq!(baseline.fg, Color::Gray);
            assert_eq!(baseline.bg, Color::DarkGray);
        }
    }

    #[test]
    fn chart_and_axis_keep_the_full_day_when_the_slice_cursor_skips_leading_zeroes() {
        // If cursor initialization also trims the chart domain, users lose the quiet start
        // of the selected day even before they activate the bucket session filter.
        let mut report = fixture_report();
        report.buckets[0].interactive_at_peak = 0;
        report.buckets[0].automated_at_peak = 0;
        let expected_start = super::format_clock(report.range_start, report.timezone);
        let selection = ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(), UTC);
        let mut app = App::new(selection, Duration::from_secs(300));
        let request = app.begin_foreground_load();
        app.apply_report(
            request.generation,
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        let area = Rect::new(0, 0, 4, 2);
        let mut buffer = Buffer::empty(area);

        render_chart(&mut buffer, area, &app, Palette::new(ColorMode::Color));
        let report = app.report().unwrap();
        let axis = timeline_axis(report, LayoutClass::Medium, 80);

        assert_eq!(app.timeline_cursor(), 1);
        assert_eq!(buffer[(0, area.height - 1)].symbol(), "·");
        assert!(axis.starts_with(&expected_start), "{axis:?}");
    }

    #[test]
    fn complete_report_renders_every_bucket_even_with_a_stale_elapsed_count() {
        // If the partial-only cutoff leaks into a complete report, a valid tail bucket is
        // rendered as future data while the rest of the dashboard describes a complete day.
        let mut report = fixture_report();
        report.partial = false;
        report.elapsed_bucket_count = 1;
        report.buckets[0].interactive_at_peak = 0;
        report.buckets[0].automated_at_peak = 0;
        report.buckets[1].interactive_at_peak = 3;
        let selection = ReportSelection::new(NaiveDate::from_ymd_opt(2026, 8, 8).unwrap(), UTC);
        let mut app = App::new(selection, Duration::from_secs(300));
        let request = app.begin_foreground_load();
        app.apply_report(
            request.generation,
            Ok(Box::new(report)),
            "2026-08-08T17:21:00Z".parse().unwrap(),
        );
        let area = Rect::new(0, 0, 2, 1);
        let mut buffer = Buffer::empty(area);

        render_chart(&mut buffer, area, &app, Palette::new(ColorMode::Monochrome));

        assert_eq!(buffer[(0, 0)].symbol(), "·");
        assert_eq!(buffer[(1, 0)].symbol(), "█");
    }

    #[test]
    fn stack_scaling_preserves_total_and_both_activity_classes() {
        // If each segment rounds independently, a short stack can exceed its scaled total and
        // clip away the Automated segment. One-cell stacks need an explicit mixed fallback.
        let scaled = scaled_stack(9, 3, 12, 5);
        assert_eq!(scaled.interactive + scaled.automated, 5);
        assert!(scaled.interactive > 0);
        assert!(scaled.automated > 0);
        assert!(!scaled.mixed);

        let compact = scaled_stack(1, 1, 7, 1);
        assert_eq!(compact.interactive + compact.automated, 0);
        assert!(compact.mixed);

        let hostile = scaled_stack(usize::MAX, usize::MAX, usize::MAX, 5);
        assert!(hostile.interactive + hostile.automated <= 5);
    }

    #[test]
    fn compressed_columns_preserve_spikes_between_sample_indices() {
        // If compression selects one source index per terminal column, any peak between those
        // indices disappears even though it contributes to the server's peak aggregate.
        let mut buckets = empty_buckets(288);
        buckets[1].interactive_at_peak = 9;

        let sample = column_bucket(&buckets, buckets.len(), 0, 80).unwrap();

        assert_eq!(sample.interactive_at_peak, 9);
    }

    #[test]
    fn column_bucket_clamps_columns_to_the_observed_cutoff() {
        // If a compressed column samples past the observed prefix, future activity can appear
        // as completed data and an entirely future column can return an invented bucket.
        let mut buckets = empty_buckets(288);
        buckets[1].interactive_at_peak = 9;

        assert_eq!(
            column_bucket(&buckets, 1, 0, 80)
                .unwrap()
                .interactive_at_peak,
            0
        );
        assert!(column_bucket(&buckets, 1, 1, 80).is_none());
    }

    #[test]
    fn column_bucket_maps_sparse_buckets_to_their_columns() {
        // If sparse upsampling uses the dense compression boundary, later terminal columns can
        // point at the wrong source bucket.
        let mut buckets = empty_buckets(2);
        buckets[1].interactive_at_peak = 9;

        assert_eq!(
            column_bucket(&buckets, 2, 2, 4)
                .unwrap()
                .interactive_at_peak,
            9
        );
    }

    #[test]
    fn observed_columns_clamps_malformed_partial_ranges() {
        // If a hostile server puts effective_end outside the requested range, cutoff math must
        // stay inside the terminal; a degenerate range must not divide by zero.
        let mut report = fixture_report();
        report.range_end = report.range_start;
        assert_eq!(observed_columns(&report, 80), 0);

        let mut before = fixture_report();
        before.effective_end = before.range_start - chrono::Duration::minutes(5);
        assert_eq!(observed_columns(&before, 80), 0);

        let mut after = fixture_report();
        after.effective_end = after.range_end + chrono::Duration::minutes(5);
        assert_eq!(observed_columns(&after, 80), 80);
    }

    #[test]
    fn axis_labels_preserve_display_width_with_combining_characters() {
        // If axis placement indexes by character count after reserving display cells, a
        // combining character in the right label can write past the row boundary.
        let labels = [
            "left".to_owned(),
            "middle".to_owned(),
            "e\u{301}".to_owned(),
        ];

        let line = place_labels(&labels, 20);

        assert_eq!(UnicodeWidthStr::width(line.as_str()), 20);
        assert!(line.ends_with("e\u{301}"));
    }

    #[test]
    fn axis_labels_drop_a_right_tick_that_cannot_keep_its_gap() {
        // If right-edge clamping overrides the required gap, adjacent clock labels merge into
        // one ambiguous token.
        let labels = ["00:00".to_owned(), "12:00".to_owned(), "00:00".to_owned()];

        let line = place_labels(&labels, 16);

        assert_eq!(line.matches("00:00").count(), 1);
        assert!(line.contains("12:00"));
    }

    fn empty_buckets(count: usize) -> Vec<Bucket> {
        let timestamp: DateTime<_> = "2026-08-08T17:00:00Z".parse().unwrap();
        vec![
            Bucket {
                start: timestamp,
                end: timestamp,
                max_agents: 0,
                agent_minutes: 0.0,
                output_tokens: 0,
                cost: Money { microdollars: 0 },
                automated_at_peak: 0,
                interactive_at_peak: 0,
            };
            count
        ]
    }

    fn fixture_report() -> Report {
        serde_json::from_str(include_str!("../../tests/fixtures/report-v5.json"))
            .expect("fixture follows the report contract")
    }
}
