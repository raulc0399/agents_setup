use crate::model::{Id, ViewState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_OPEN_VIEWS: usize = 32;

/// Private arrangement and viewport state; never part of published documents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ViewLayout {
    pub tabs: Vec<Id>,
    #[serde(default)]
    pub closed: Vec<Id>,
    pub active: Option<Id>,
    pub split: Option<Id>,
    pub presentation: BTreeMap<Id, Presentation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Presentation {
    pub focus: Option<Id>,
    pub pan_x: i32,
    pub pan_y: i32,
    pub list_offset: usize,
    pub detail_scroll: u16,
    pub detail_page: usize,
}

impl ViewLayout {
    pub fn reconcile(&mut self, views: &BTreeMap<Id, ViewState>) {
        let mut seen = std::collections::BTreeSet::new();
        self.tabs
            .retain(|id| views.contains_key(id) && seen.insert(*id));
        self.tabs.truncate(MAX_OPEN_VIEWS);
        self.closed
            .retain(|id| views.contains_key(id) && !self.tabs.contains(id));
        self.presentation.retain(|id, _| views.contains_key(id));
        if !self.active.is_some_and(|id| self.tabs.contains(&id)) {
            self.active = self.tabs.first().copied();
        }
        if !self
            .split
            .is_some_and(|id| self.tabs.contains(&id) && Some(id) != self.active)
        {
            self.split = None;
        }
    }
}
