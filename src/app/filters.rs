// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::cmp::Ordering;

use chrono::NaiveDate;

use crate::wire::Automation;

use super::{App, Loadable};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Date,
    Project,
    Agent,
    Machine,
    Automation,
    Timeline,
    Sessions,
    Breakdowns,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataKind {
    Projects,
    Agents,
    Machines,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactRegion {
    Sessions,
    Breakdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FilterChoice {
    All,
    Text(String),
    Automation(Automation),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FilterItem {
    label: String,
    choice: FilterChoice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilterPopup {
    pub selected: usize,
    pub query: String,
    focus: Focus,
    items: Vec<FilterItem>,
    visible: Vec<usize>,
    initial_selected: usize,
}

impl FilterPopup {
    pub fn is_searchable(&self) -> bool {
        self.focus == Focus::Project
    }

    pub fn labels(&self) -> impl ExactSizeIterator<Item = &str> {
        self.visible
            .iter()
            .map(|index| self.items[*index].label.as_str())
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }
}

impl App {
    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        match focus {
            Focus::Sessions => self.compact_region = CompactRegion::Sessions,
            Focus::Breakdowns => self.compact_region = CompactRegion::Breakdown,
            Focus::Date
            | Focus::Project
            | Focus::Agent
            | Focus::Machine
            | Focus::Automation
            | Focus::Timeline => {}
        }
        self.popup = None;
    }

    pub fn popup(&self) -> Option<&FilterPopup> {
        self.popup.as_ref()
    }

    pub fn set_project(&mut self, project: Option<String>) {
        self.selection.project = project;
    }

    pub fn set_agent(&mut self, agent: Option<String>) {
        self.selection.agent = agent;
    }

    pub fn set_machine(&mut self, machine: Option<String>) {
        self.selection.machine = machine;
    }

    pub(crate) fn move_focus(&mut self, delta: isize) {
        const ORDER: [Focus; 8] = [
            Focus::Date,
            Focus::Project,
            Focus::Agent,
            Focus::Machine,
            Focus::Automation,
            Focus::Timeline,
            Focus::Sessions,
            Focus::Breakdowns,
        ];
        let current = ORDER
            .iter()
            .position(|focus| *focus == self.focus)
            .expect("closed focus value");
        let next = (current as isize + delta).rem_euclid(ORDER.len() as isize) as usize;
        self.set_focus(ORDER[next]);
    }

    pub(crate) fn move_date(&mut self, direction: Ordering) -> bool {
        let next = match direction {
            Ordering::Less => self.selection.date.pred_opt(),
            Ordering::Equal => Some(self.selection.date),
            Ordering::Greater => self.selection.date.succ_opt(),
        };
        let Some(next) = next else {
            return false;
        };
        let changed = next != self.selection.date;
        self.selection.date = next;
        changed
    }

    pub(crate) fn clear_focused_filter(&mut self, today: NaiveDate) -> bool {
        match self.focus {
            Focus::Date => replace_if_changed(&mut self.selection.date, today),
            Focus::Project => take_if_some(&mut self.selection.project),
            Focus::Agent => take_if_some(&mut self.selection.agent),
            Focus::Machine => take_if_some(&mut self.selection.machine),
            Focus::Automation => {
                replace_if_changed(&mut self.selection.automation, Automation::All)
            }
            Focus::Timeline | Focus::Sessions | Focus::Breakdowns => false,
        }
    }

    pub(crate) fn open_filter_popup(&mut self) -> bool {
        let (items, selected) = match self.focus {
            Focus::Project => {
                let Loadable::Ready(projects) = &self.projects else {
                    return false;
                };
                text_popup(
                    projects.iter().map(|project| project.name.as_str()),
                    self.selection.project.as_deref(),
                )
            }
            Focus::Agent => {
                let Loadable::Ready(agents) = &self.agents else {
                    return false;
                };
                text_popup(
                    agents.iter().map(|agent| agent.name.as_str()),
                    self.selection.agent.as_deref(),
                )
            }
            Focus::Machine => {
                let Loadable::Ready(machines) = &self.machines else {
                    return false;
                };
                text_popup(
                    machines.iter().map(String::as_str),
                    self.selection.machine.as_deref(),
                )
            }
            Focus::Automation => {
                let values = [
                    ("All", Automation::All),
                    ("Interactive", Automation::Interactive),
                    ("Automated", Automation::Automated),
                ];
                let selected = values
                    .iter()
                    .position(|(_, value)| *value == self.selection.automation)
                    .expect("closed automation value");
                let items = values
                    .iter()
                    .map(|(label, value)| FilterItem {
                        label: (*label).to_owned(),
                        choice: FilterChoice::Automation(*value),
                    })
                    .collect();
                (items, selected)
            }
            Focus::Date | Focus::Timeline | Focus::Sessions | Focus::Breakdowns => return false,
        };
        let visible = (0..items.len()).collect();
        self.popup = Some(FilterPopup {
            selected,
            focus: self.focus,
            items,
            visible,
            query: String::new(),
            initial_selected: selected,
        });
        true
    }

    pub(crate) fn move_popup(&mut self, delta: isize) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        popup.selected = popup
            .selected
            .saturating_add_signed(delta)
            .min(popup.len().saturating_sub(1));
    }

    pub(crate) fn close_popup(&mut self) {
        self.popup = None;
    }

    pub(crate) fn edit_popup_query(&mut self, edit: PopupQueryEdit) {
        let Some(popup) = &mut self.popup else {
            return;
        };
        if !popup.is_searchable() {
            return;
        }
        match edit {
            PopupQueryEdit::Push(character) => popup.query.push(character),
            PopupQueryEdit::Pop => {
                popup.query.pop();
            }
        }
        refresh_project_results(popup);
    }

    pub(crate) fn accept_popup(&mut self) -> bool {
        let Some(popup) = self.popup.as_ref() else {
            return false;
        };
        let Some(choice) = popup
            .visible
            .get(popup.selected)
            .and_then(|index| popup.items.get(*index))
            .map(|item| item.choice.clone())
        else {
            return false;
        };
        let focus = popup.focus;
        self.popup = None;
        match (focus, choice) {
            (Focus::Project, FilterChoice::All) => take_if_some(&mut self.selection.project),
            (Focus::Project, FilterChoice::Text(value)) => {
                replace_if_changed(&mut self.selection.project, Some(value))
            }
            (Focus::Agent, FilterChoice::All) => take_if_some(&mut self.selection.agent),
            (Focus::Agent, FilterChoice::Text(value)) => {
                replace_if_changed(&mut self.selection.agent, Some(value))
            }
            (Focus::Machine, FilterChoice::All) => take_if_some(&mut self.selection.machine),
            (Focus::Machine, FilterChoice::Text(value)) => {
                replace_if_changed(&mut self.selection.machine, Some(value))
            }
            (Focus::Automation, FilterChoice::Automation(value)) => {
                replace_if_changed(&mut self.selection.automation, value)
            }
            _ => false,
        }
    }

    pub(crate) fn failed_metadata_for_focus(&self) -> Option<MetadataKind> {
        match self.focus {
            Focus::Project if matches!(self.projects, Loadable::Failed(_)) => {
                Some(MetadataKind::Projects)
            }
            Focus::Agent if matches!(self.agents, Loadable::Failed(_)) => {
                Some(MetadataKind::Agents)
            }
            Focus::Machine if matches!(self.machines, Loadable::Failed(_)) => {
                Some(MetadataKind::Machines)
            }
            _ => None,
        }
    }

    pub(crate) fn focused_filter_is_ready(&self) -> bool {
        match self.focus {
            Focus::Project => matches!(self.projects, Loadable::Ready(_)),
            Focus::Agent => matches!(self.agents, Loadable::Ready(_)),
            Focus::Machine => matches!(self.machines, Loadable::Ready(_)),
            Focus::Automation => true,
            Focus::Date | Focus::Timeline | Focus::Sessions | Focus::Breakdowns => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PopupQueryEdit {
    Push(char),
    Pop,
}

fn refresh_project_results(popup: &mut FilterPopup) {
    if popup.query.is_empty() {
        popup.visible = (0..popup.items.len()).collect();
        popup.selected = popup.initial_selected;
        return;
    }

    let mut matches = popup
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match &item.choice {
            FilterChoice::Text(_) => {
                fuzzy_score(&item.label, &popup.query).map(|score| (score, index))
            }
            FilterChoice::All | FilterChoice::Automation(_) => None,
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    popup.visible = matches.into_iter().map(|(_, index)| index).collect();
    popup.selected = 0;
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let candidate = candidate.to_lowercase().chars().collect::<Vec<_>>();
    let query = query.to_lowercase().chars().collect::<Vec<_>>();
    let mut score = 0_i64;
    let mut cursor = 0;
    let mut previous = None;
    for wanted in query {
        let offset = candidate[cursor..]
            .iter()
            .position(|character| *character == wanted)?;
        let index = cursor + offset;
        score += 20 - index.min(20) as i64;
        if index == 0 || !candidate[index - 1].is_alphanumeric() {
            score += 12;
        }
        if previous.is_some_and(|previous| previous + 1 == index) {
            score += 8;
        }
        previous = Some(index);
        cursor = index + 1;
    }
    Some(score)
}

fn text_popup<'a>(
    values: impl Iterator<Item = &'a str>,
    current: Option<&str>,
) -> (Vec<FilterItem>, usize) {
    let mut items = vec![FilterItem {
        label: "All".to_owned(),
        choice: FilterChoice::All,
    }];
    for value in values {
        items.push(FilterItem {
            label: value.to_owned(),
            choice: FilterChoice::Text(value.to_owned()),
        });
    }
    let selected = current
        .and_then(|current| {
            items.iter().position(
                |item| matches!(&item.choice, FilterChoice::Text(value) if value == current),
            )
        })
        .unwrap_or(0);
    (items, selected)
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> bool {
    if *target == value {
        return false;
    }
    *target = value;
    true
}

fn take_if_some<T>(target: &mut Option<T>) -> bool {
    target.take().is_some()
}
