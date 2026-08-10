use ratatui::layout::Rect;

use crate::app::{App, CompactRegion};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutClass {
    Wide,
    Medium,
    Compact,
    TooSmall,
}

impl LayoutClass {
    pub fn for_size(width: u16, height: u16) -> Self {
        match (width, height) {
            (width, height) if width >= 160 && height >= 45 => Self::Wide,
            (width, height) if width >= 100 && height >= 32 => Self::Medium,
            (width, height) if width >= 80 && height >= 24 => Self::Compact,
            _ => Self::TooSmall,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FramePlan {
    area: Rect,
    class: LayoutClass,
    header: Option<Rect>,
    summary: Option<Rect>,
    timeline: Option<Rect>,
    sessions: Option<Rect>,
    breakdowns: Option<Rect>,
    compact_rail: Option<Rect>,
    footer: Option<Rect>,
}

impl FramePlan {
    pub fn new(app: &App, area: Rect) -> Self {
        let class = LayoutClass::for_size(area.width, area.height);
        if class == LayoutClass::TooSmall {
            return Self {
                area,
                class,
                header: None,
                summary: None,
                timeline: None,
                sessions: None,
                breakdowns: None,
                compact_rail: None,
                footer: None,
            };
        }

        let footer = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        );
        let mut cursor = area.y;
        let header = take(area, &mut cursor, 2);
        let summary = take(area, &mut cursor, 5);
        let timeline_height = match class {
            LayoutClass::Wide => 9,
            LayoutClass::Medium => 7,
            LayoutClass::Compact => 5,
            LayoutClass::TooSmall => unreachable!(),
        };
        let timeline = take(area, &mut cursor, timeline_height);
        let bottom = footer.y;

        let (sessions, breakdowns, compact_rail) = match class {
            LayoutClass::Wide | LayoutClass::Medium => {
                let breakdown_height = if class == LayoutClass::Wide { 10 } else { 9 };
                let breakdown_y = bottom.saturating_sub(breakdown_height);
                let sessions = Rect::new(
                    area.x,
                    cursor,
                    area.width,
                    breakdown_y.saturating_sub(cursor),
                );
                let breakdowns = Rect::new(
                    area.x,
                    breakdown_y,
                    area.width,
                    bottom.saturating_sub(breakdown_y),
                );
                (Some(sessions), Some(breakdowns), None)
            }
            LayoutClass::Compact => {
                let rail = Rect::new(area.x, cursor, area.width, 1);
                let data = Rect::new(
                    area.x,
                    cursor + 1,
                    area.width,
                    bottom.saturating_sub(cursor + 1),
                );
                match app.compact_region() {
                    CompactRegion::Sessions => (Some(data), None, Some(rail)),
                    CompactRegion::Breakdown => (None, Some(data), Some(rail)),
                }
            }
            LayoutClass::TooSmall => unreachable!(),
        };

        Self {
            area,
            class,
            header: Some(header),
            summary: Some(summary),
            timeline: Some(timeline),
            sessions,
            breakdowns,
            compact_rail,
            footer: Some(footer),
        }
    }

    pub fn class(&self) -> LayoutClass {
        self.class
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    pub fn sessions(&self) -> Option<Rect> {
        self.sessions
    }

    pub fn session_viewport_rows(&self) -> Option<usize> {
        self.sessions
            .map(|area| super::sessions::viewport_rows(area, self.class))
    }

    pub fn breakdowns(&self) -> Option<Rect> {
        self.breakdowns
    }

    pub(super) fn header(&self) -> Option<Rect> {
        self.header
    }

    pub(super) fn summary(&self) -> Option<Rect> {
        self.summary
    }

    pub(super) fn timeline(&self) -> Option<Rect> {
        self.timeline
    }

    pub(super) fn compact_rail(&self) -> Option<Rect> {
        self.compact_rail
    }

    pub(super) fn footer(&self) -> Option<Rect> {
        self.footer
    }
}

fn take(area: Rect, cursor: &mut u16, height: u16) -> Rect {
    let consumed = cursor.saturating_sub(area.y);
    let available = area.height.saturating_sub(consumed);
    let height = height.min(available);
    let rect = Rect::new(area.x, *cursor, area.width, height);
    *cursor = cursor.saturating_add(height);
    rect
}
