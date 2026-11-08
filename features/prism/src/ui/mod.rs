pub mod diagram;
pub mod list;
pub mod views;

use crate::actions::{ContentAction, PrepareQuestion};
use crate::model::*;
use crate::store::Store;
use anyhow::{Result, bail};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const EVENT_WAIT: Duration = Duration::from_millis(50);
const DETAIL_PAGE_CHARACTERS: usize = 4_096;
const DIAGRAM_PAN_STEP: i32 = 4;
const DIAGRAM_MOVE_STEP: i32 = 2;

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub generation: u64,
    pub connected: bool,
    /// Only capabilities verified for this exact live binding may appear here.
    pub send_modes: Vec<DeliveryMode>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Browse,
    Question,
    Filter,
}

pub struct UiState {
    pub workspace: Workspace,
    pub documents: Vec<Document>,
    pub view: Option<ViewState>,
    pub focus: Option<Id>,
    pub input_mode: InputMode,
    pub delivery_mode: DeliveryMode,
    pub message: String,
    pub questions: Vec<Question>,
    list_state: ListState,
    list_area: Rect,
    detail_scroll: u16,
    detail_page: usize,
    editor_cursor: usize,
    editor_generation: Option<u64>,
    undo: BTreeMap<Id, Vec<Id>>,
    show_help: bool,
    pub layout: views::ViewLayout,
    saved_views: BTreeMap<Id, ViewState>,
    diagram_state: diagram::DiagramState,
    content_area: Rect,
    split_area: Rect,
    tab_areas: Vec<(Rect, Id)>,
}

impl UiState {
    pub fn new(store: &mut Store, workspace: Workspace) -> Result<Self> {
        let mut state = Self {
            workspace: workspace.clone(),
            documents: Vec::new(),
            view: None,
            focus: None,
            input_mode: InputMode::Browse,
            delivery_mode: DeliveryMode::Share,
            message: String::new(),
            questions: Vec::new(),
            list_state: ListState::default(),
            list_area: Rect::default(),
            detail_scroll: 0,
            detail_page: 0,
            editor_cursor: 0,
            editor_generation: None,
            undo: BTreeMap::new(),
            show_help: false,
            layout: store
                .ui_state(workspace.id)?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default(),
            saved_views: store
                .views(workspace.id)?
                .into_iter()
                .map(|view| (view.id, view))
                .collect(),
            diagram_state: diagram::DiagramState::default(),
            content_area: Rect::default(),
            split_area: Rect::default(),
            tab_areas: Vec::new(),
        };
        state.refresh(store)?;
        Ok(state)
    }

    pub fn document(&self) -> Option<&Document> {
        let id = self.view.as_ref()?.document_id;
        self.documents.iter().find(|document| document.id == id)
    }

    pub fn rows(&self) -> Vec<list::Row> {
        match (self.document(), &self.view) {
            (Some(document), Some(view)) => list::rows(document, view),
            _ => Vec::new(),
        }
    }

    pub fn selection_stale(&self) -> bool {
        match (
            self.document(),
            self.view.as_ref().and_then(|view| view.selection.as_ref()),
        ) {
            (Some(document), Some(selection)) => selection.revision != document.revision,
            _ => false,
        }
    }

    pub fn refresh(&mut self, store: &mut Store) -> Result<()> {
        self.documents = store.documents(self.workspace.id)?;
        self.questions = store.questions(self.workspace.id)?;
        self.saved_views = store
            .views(self.workspace.id)?
            .into_iter()
            .map(|view| (view.id, view))
            .collect();
        let initial = self.layout.tabs.is_empty() && self.view.is_none();
        if initial {
            self.layout.tabs = self
                .saved_views
                .keys()
                .copied()
                .take(views::MAX_OPEN_VIEWS)
                .collect();
        }
        for document in &self.documents {
            if self.layout.tabs.len() >= views::MAX_OPEN_VIEWS {
                break;
            }
            if !self
                .saved_views
                .values()
                .any(|view| view.document_id == document.id)
            {
                let view = Self::empty_view(document.id);
                store.save_view(self.workspace.id, &view)?;
                self.layout.tabs.push(view.id);
                self.saved_views.insert(view.id, view);
            }
        }
        self.layout.reconcile(&self.saved_views);
        if self.view.is_none() {
            if let Some(id) = self.layout.active {
                self.activate_view(id)?;
            }
        } else if let Some(view) = &self.view {
            self.view = self.saved_views.get(&view.id).cloned();
        }
        if let Some(view) = &self.view {
            let text = if self.input_mode == InputMode::Filter {
                &view.filter
            } else {
                &view.draft
            };
            self.editor_cursor = text.floor_char_boundary(self.editor_cursor.min(text.len()));
        }
        self.reconcile_focus();
        self.persist_layout(store)?;
        Ok(())
    }

    fn empty_view(document_id: Id) -> ViewState {
        ViewState {
            id: Id::new_v4(),
            document_id,
            kind: ViewKind::List,
            selection: None,
            positions: BTreeMap::new(),
            hidden: BTreeSet::new(),
            filter: String::new(),
            draft: String::new(),
        }
    }

    fn remember_view(&mut self) {
        if let Some(view) = &self.view {
            self.saved_views.insert(view.id, view.clone());
            self.layout.presentation.insert(
                view.id,
                views::Presentation {
                    focus: self.focus,
                    pan_x: self.diagram_state.pan_x,
                    pan_y: self.diagram_state.pan_y,
                    list_offset: self.list_state.offset(),
                    detail_scroll: self.detail_scroll,
                    detail_page: self.detail_page,
                },
            );
        }
    }

    fn persist_layout(&mut self, store: &mut Store) -> Result<()> {
        self.remember_view();
        store.save_ui_state(self.workspace.id, &serde_json::to_value(&self.layout)?)?;
        Ok(())
    }

    fn activate_view(&mut self, id: Id) -> Result<()> {
        self.remember_view();
        let view = self
            .saved_views
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("View is unavailable"))?;
        let presentation = self
            .layout
            .presentation
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if self.layout.split == Some(id) {
            self.layout.split = self.layout.active;
        }
        self.layout.active = Some(id);
        self.focus = presentation.focus.or_else(|| {
            view.selection
                .as_ref()
                .and_then(|selection| selection.ids.first().copied())
        });
        self.editor_cursor = view.draft.len();
        self.view = Some(view);
        self.list_state = ListState::default().with_offset(presentation.list_offset);
        self.diagram_state = diagram::DiagramState {
            pan_x: presentation.pan_x,
            pan_y: presentation.pan_y,
        };
        self.detail_scroll = presentation.detail_scroll;
        self.detail_page = presentation.detail_page;
        self.reconcile_focus();
        Ok(())
    }

    fn duplicate_view(&mut self, store: &mut Store) -> Result<()> {
        if self.layout.tabs.len() >= views::MAX_OPEN_VIEWS {
            bail!(
                "At most {} views may be open; close a view first",
                views::MAX_OPEN_VIEWS
            );
        }
        let original = self
            .view
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No document published"))?;
        let mut view = Self::empty_view(original.document_id);
        view.kind = original.kind.clone();
        let id = view.id;
        store.save_view(self.workspace.id, &view)?;
        self.saved_views.insert(id, view);
        self.layout.tabs.push(id);
        self.activate_view(id)
    }

    fn cycle_view(&mut self, backwards: bool) -> Result<()> {
        if self.layout.tabs.is_empty() {
            return Ok(());
        }
        let index = self
            .layout
            .tabs
            .iter()
            .position(|id| Some(*id) == self.layout.active)
            .unwrap_or(0);
        let count = self.layout.tabs.len();
        let next = if backwards {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        };
        self.activate_view(self.layout.tabs[next])
    }

    fn close_view(&mut self) -> Result<()> {
        if self.layout.tabs.len() <= 1 {
            bail!("Keep at least one view open");
        }
        let active = self.layout.active;
        self.layout.tabs.retain(|id| Some(*id) != active);
        if let Some(id) = active {
            self.layout.closed.push(id);
        }
        let next = self.layout.split.unwrap_or(self.layout.tabs[0]);
        self.layout.split = None;
        self.activate_view(next)?;
        self.message =
            "View closed; published content and its saved private draft are preserved".into();
        Ok(())
    }

    fn reopen_view(&mut self) -> Result<()> {
        if self.layout.tabs.len() >= views::MAX_OPEN_VIEWS {
            bail!("Close a view before reopening another");
        }
        let id = self
            .layout
            .closed
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No closed view to reopen"))?;
        self.layout.tabs.push(id);
        self.activate_view(id)
    }

    fn toggle_split(&mut self, store: &mut Store) -> Result<()> {
        if self.layout.split.take().is_some() {
            return Ok(());
        }
        let original = self.layout.active;
        if self.layout.tabs.len() == 1 {
            self.duplicate_view(store)?;
            self.layout.split = original;
        } else {
            self.layout.split = self
                .layout
                .tabs
                .iter()
                .copied()
                .find(|id| Some(*id) != original);
        }
        Ok(())
    }

    fn diagram_layout(&self) -> Option<diagram::DiagramLayout> {
        let view = self.view.as_ref()?;
        (view.kind == ViewKind::Diagram).then(|| {
            diagram::DiagramLayout::new(self.document().expect("view document exists"), view)
        })
    }

    fn visible_ids(&self) -> Vec<Id> {
        self.diagram_layout().map_or_else(
            || self.rows().iter().map(|row| row.id).collect(),
            |layout| layout.visible_ids(),
        )
    }

    fn reconcile_focus(&mut self) {
        let ids = self.visible_ids();
        let index = ids
            .iter()
            .position(|id| Some(*id) == self.focus)
            .or_else(|| (!ids.is_empty()).then_some(0));
        self.focus = index.map(|index| ids[index]);
        self.list_state.select(index);
    }

    fn save_view(&mut self, store: &mut Store, update: impl FnOnce(&mut ViewState)) -> Result<()> {
        let mut view = self
            .view
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No document published"))?;
        update(&mut view);
        store.save_view(self.workspace.id, &view)?;
        self.view = Some(view);
        self.reconcile_focus();
        Ok(())
    }

    fn move_focus(&mut self, delta: isize) {
        if let Some(layout) = self.diagram_layout() {
            self.focus = layout.focus_navigation(self.focus, delta);
            if let Some(id) = self.focus {
                self.diagram_state.reveal(id, &layout, self.content_area);
            }
            self.detail_scroll = 0;
            self.detail_page = 0;
            return;
        }
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let index = self
            .list_state
            .selected()
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(rows.len() - 1);
        self.focus = Some(rows[index].id);
        self.list_state.select(Some(index));
        self.detail_scroll = 0;
        self.detail_page = 0;
    }

    fn selection(&self) -> Result<Selection> {
        if self.selection_stale() {
            bail!("Selection is stale; press r to clear, then select the current revision");
        }
        self.view
            .as_ref()
            .and_then(|view| view.selection.clone())
            .filter(|selection| !selection.ids.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Select content with Space first"))
    }

    fn toggle_selection(&mut self, store: &mut Store) -> Result<()> {
        if self.selection_stale() {
            bail!("Selection is stale; press r to clear before selecting again");
        }
        let Some(id) = self.focus else {
            return Ok(());
        };
        let Some(document) = self.document() else {
            return Ok(());
        };
        let mut selection = self
            .view
            .as_ref()
            .and_then(|view| view.selection.clone())
            .unwrap_or(Selection {
                document_id: document.id,
                revision: document.revision,
                ids: Vec::new(),
            });
        if selection.ids.contains(&id) {
            selection.ids.retain(|value| *value != id);
        } else {
            selection.ids.push(id);
        }
        self.save_view(store, |view| {
            view.selection = (!selection.ids.is_empty()).then_some(selection)
        })
    }

    fn apply(&mut self, store: &mut Store, archive: bool) -> Result<()> {
        let selection = self.selection()?;
        let action = if archive {
            ContentAction::Archive { ids: selection.ids }
        } else {
            ContentAction::Accept { ids: selection.ids }
        };
        let result = store.apply(
            self.workspace.id,
            selection.document_id,
            selection.revision,
            &action,
        )?;
        self.undo
            .entry(selection.document_id)
            .or_default()
            .push(result.undo_id);
        // Clear the old revision only after the shared mutation has committed.
        self.save_view(store, |view| view.selection = None)?;
        self.refresh(store)?;
        self.message = if archive {
            "Archived selected content; u undoes"
        } else {
            "Accepted selected content; acceptance does not authorize tools"
        }
        .into();
        Ok(())
    }

    fn undo(&mut self, store: &mut Store) -> Result<()> {
        let document = self
            .document()
            .ok_or_else(|| anyhow::anyhow!("No document"))?;
        let (id, revision) = (document.id, document.revision);
        let undo_id = self
            .undo
            .get(&id)
            .and_then(|records| records.last())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("No action to undo in this UI session"))?;
        store.undo(self.workspace.id, id, revision, undo_id)?;
        self.undo.get_mut(&id).expect("undo record exists").pop();
        self.refresh(store)?;
        self.message = "Action undone".into();
        Ok(())
    }

    pub fn available_modes(connection: &ConnectionInfo) -> Vec<DeliveryMode> {
        let mut modes = Vec::new();
        if connection.connected && connection.generation != 0 {
            for mode in &connection.send_modes {
                if !modes.contains(mode) && *mode != DeliveryMode::Share {
                    modes.push(mode.clone());
                }
            }
        }
        modes.push(DeliveryMode::Share);
        modes
    }

    fn send(&mut self, store: &mut Store, connection: &ConnectionInfo) -> Result<()> {
        if self.editor_generation != Some(connection.generation) || connection.generation == 0 {
            bail!("Binding changed or is unavailable; Esc then e to review the current binding");
        }
        if !Self::available_modes(connection).contains(&self.delivery_mode) {
            bail!(
                "Selected delivery mode is unavailable; use Ctrl-N to choose Share or a verified mode"
            );
        }
        let selection = self.selection()?;
        let view = self
            .view
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No private view"))?;
        let question = store.prepare_question_and_clear_draft(
            self.workspace.id,
            &PrepareQuestion {
                request_id: Id::new_v4(),
                generation: connection.generation,
                selection,
                text: view.draft.clone(),
                mode: self.delivery_mode.clone(),
            },
            view.id,
        )?;
        if let Some(view) = &mut self.view {
            view.draft.clear();
        }
        self.questions.push(question);
        self.input_mode = InputMode::Browse;
        self.editor_generation = None;
        self.editor_cursor = 0;
        self.message = if self.delivery_mode == DeliveryMode::Share {
            "Shared snapshot; invoke Prism retrieval in the bound CLI. No native turn has started."
        } else {
            "Persisted pending question; awaiting bridge dispatch"
        }
        .into();
        Ok(())
    }

    /// Returns true only for an explicit exit; action errors remain visible in the UI.
    pub fn handle_event(
        &mut self,
        store: &mut Store,
        event: Event,
        connection: &ConnectionInfo,
    ) -> bool {
        match self.process_event(store, event, connection) {
            Ok(exit) => {
                if let Err(error) = self.persist_layout(store) {
                    self.message = format!("Error saving private arrangement: {error}");
                }
                exit
            }
            Err(error) => {
                self.message = format!("Error: {error}");
                false
            }
        }
    }

    fn process_event(
        &mut self,
        store: &mut Store,
        event: Event,
        connection: &ConnectionInfo,
    ) -> Result<bool> {
        if let Event::Mouse(mouse) = event {
            if self.input_mode != InputMode::Browse || self.show_help {
                return Ok(false);
            }
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                if let Some((_, id)) = self
                    .tab_areas
                    .iter()
                    .find(|(area, _)| area.contains((mouse.column, mouse.row).into()))
                {
                    self.activate_view(*id)?;
                    return Ok(false);
                }
                if self.split_area.contains((mouse.column, mouse.row).into()) {
                    if let Some(id) = self.layout.split {
                        self.activate_view(id)?;
                    }
                    return Ok(false);
                }
            }
            if let Some(layout) = self.diagram_layout() {
                if self.content_area.contains((mouse.column, mouse.row).into()) {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            self.diagram_state
                                .pan(0, DIAGRAM_PAN_STEP, &layout, self.content_area)
                        }
                        MouseEventKind::ScrollUp => {
                            self.diagram_state
                                .pan(0, -DIAGRAM_PAN_STEP, &layout, self.content_area)
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            if let Some(id) = self.diagram_state.hit_test(
                                mouse.column,
                                mouse.row,
                                self.content_area,
                                &layout,
                            ) {
                                self.focus = Some(id);
                                self.detail_scroll = 0;
                                self.detail_page = 0;
                                self.toggle_selection(store)?;
                            }
                        }
                        _ => {}
                    }
                }
                return Ok(false);
            }
            if self.list_area.contains((mouse.column, mouse.row).into()) {
                match mouse.kind {
                    MouseEventKind::ScrollDown => self.move_focus(1),
                    MouseEventKind::ScrollUp => self.move_focus(-1),
                    MouseEventKind::Down(MouseButton::Left) => {
                        let index =
                            self.list_state.offset() + usize::from(mouse.row - self.list_area.y);
                        if let Some(row) = self.rows().get(index) {
                            self.focus = Some(row.id);
                            self.list_state.select(Some(index));
                            self.detail_scroll = 0;
                            self.detail_page = 0;
                            self.toggle_selection(store)?;
                        }
                    }
                    _ => {}
                }
            }
            return Ok(false);
        }
        let Event::Key(key) = event else {
            return Ok(false);
        };
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }
        if self.input_mode != InputMode::Browse {
            self.edit(store, key, connection)?;
            return Ok(false);
        }
        if self.show_help {
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc => {
                    self.show_help = false;
                    self.detail_scroll = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.detail_scroll = self.detail_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(5),
                KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
                KeyCode::Char('q') => return Ok(true),
                _ => {}
            }
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('t') => self.duplicate_view(store)?,
                KeyCode::Char('w') => self.close_view()?,
                KeyCode::Char('r') => self.reopen_view()?,
                KeyCode::Char('\\') => self.toggle_split(store)?,
                _ => return Ok(false),
            }
            return Ok(false);
        }
        if let Some(layout) = self.diagram_layout() {
            let delta = match key.code {
                KeyCode::Left => Some((-1, 0)),
                KeyCode::Right => Some((1, 0)),
                KeyCode::Up => Some((0, -1)),
                KeyCode::Down => Some((0, 1)),
                _ => None,
            };
            if let Some((dx, dy)) = delta {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    if let Some(id) = self.focus
                        && let Some(position) = layout.position(id)
                    {
                        self.save_view(store, |view| {
                            view.positions.insert(
                                id,
                                Position {
                                    x: position.x.saturating_add(dx * DIAGRAM_MOVE_STEP),
                                    y: position.y.saturating_add(dy * DIAGRAM_MOVE_STEP),
                                },
                            );
                        })?;
                        self.move_focus(0);
                    }
                    return Ok(false);
                }
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.diagram_state.pan(
                        dx * DIAGRAM_PAN_STEP,
                        dy * DIAGRAM_PAN_STEP,
                        &layout,
                        self.content_area,
                    );
                    return Ok(false);
                }
            }
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') => {
                self.show_help = true;
                self.detail_scroll = 0;
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j') => self.move_focus(1),
            KeyCode::Up | KeyCode::Left | KeyCode::Char('k') => self.move_focus(-1),
            KeyCode::Home => self.move_focus(isize::MIN),
            KeyCode::End => self.move_focus(isize::MAX),
            KeyCode::Char(' ') => self.toggle_selection(store)?,
            KeyCode::Char('r') => self.save_view(store, |view| view.selection = None)?,
            KeyCode::Char('A') => {
                let document = self
                    .document()
                    .ok_or_else(|| anyhow::anyhow!("No document"))?;
                let selection = Selection {
                    document_id: document.id,
                    revision: document.revision,
                    ids: self.visible_ids(),
                };
                self.save_view(store, |view| {
                    view.selection = (!selection.ids.is_empty()).then_some(selection)
                })?;
            }
            KeyCode::Char('a') => self.apply(store, false)?,
            KeyCode::Char('x') => self.apply(store, true)?,
            KeyCode::Char('u') => self.undo(store)?,
            KeyCode::Char('h') => {
                let ids = self.selection()?.ids;
                self.save_view(store, |view| {
                    view.hidden.extend(ids);
                    view.selection = None;
                })?;
            }
            KeyCode::Char('H') => self.save_view(store, |view| view.hidden.clear())?,
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Filter;
                self.editor_cursor = self.view.as_ref().map_or(0, |view| view.filter.len());
            }
            KeyCode::Char('e') => {
                self.selection()?;
                self.input_mode = InputMode::Question;
                self.editor_cursor = self.view.as_ref().map_or(0, |view| view.draft.len());
                self.editor_generation = Some(connection.generation);
            }
            KeyCode::Tab | KeyCode::BackTab => self.cycle_view(key.code == KeyCode::BackTab)?,
            KeyCode::F(6) => {
                if let Some(id) = self.layout.split {
                    self.activate_view(id)?;
                }
            }
            KeyCode::Char('v') => {
                self.save_view(store, |view| {
                    view.kind = if view.kind == ViewKind::List {
                        ViewKind::Diagram
                    } else {
                        ViewKind::List
                    }
                })?;
                self.move_focus(0);
            }
            KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(5),
            KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(5),
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let pages = self
                    .details_text()
                    .chars()
                    .count()
                    .div_ceil(DETAIL_PAGE_CHARACTERS)
                    .max(1);
                self.detail_page = if key.code == KeyCode::Char(']') {
                    self.detail_page.saturating_add(1).min(pages - 1)
                } else {
                    self.detail_page.saturating_sub(1)
                };
                self.detail_scroll = 0;
            }
            _ => {}
        }
        Ok(false)
    }

    fn edit(
        &mut self,
        store: &mut Store,
        key: KeyEvent,
        connection: &ConnectionInfo,
    ) -> Result<()> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            self.input_mode = InputMode::Browse;
            self.editor_generation = None;
            return Ok(());
        }
        if self.input_mode == InputMode::Question && control {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('s')) {
                // Repeated key events must never create a second request.
                if key.kind == KeyEventKind::Press {
                    self.send(store, connection)?;
                }
                return Ok(());
            }
            if key.code == KeyCode::Char('n') {
                let modes = Self::available_modes(connection);
                let index = modes
                    .iter()
                    .position(|mode| mode == &self.delivery_mode)
                    .map_or(0, |index| (index + 1) % modes.len());
                self.delivery_mode = modes[index].clone();
                return Ok(());
            }
        }
        let filter = self.input_mode == InputMode::Filter;
        let Some(view) = &self.view else {
            return Ok(());
        };
        let mut text = if filter {
            view.filter.clone()
        } else {
            view.draft.clone()
        };
        let mut cursor = text.floor_char_boundary(self.editor_cursor.min(text.len()));
        match key.code {
            KeyCode::Left => {
                cursor = text[..cursor]
                    .char_indices()
                    .next_back()
                    .map_or(0, |(index, _)| index)
            }
            KeyCode::Right => cursor += text[cursor..].chars().next().map_or(0, char::len_utf8),
            KeyCode::Home => cursor = text[..cursor].rfind('\n').map_or(0, |index| index + 1),
            KeyCode::End => cursor += text[cursor..].find('\n').unwrap_or(text.len() - cursor),
            KeyCode::Backspace => {
                if let Some((index, _)) = text[..cursor].char_indices().next_back() {
                    text.drain(index..cursor);
                    cursor = index;
                }
            }
            KeyCode::Delete => {
                if let Some(character) = text[cursor..].chars().next() {
                    text.drain(cursor..cursor + character.len_utf8());
                }
            }
            KeyCode::Enter if filter => {
                self.input_mode = InputMode::Browse;
                return Ok(());
            }
            KeyCode::Enter => {
                text.insert(cursor, '\n');
                cursor += 1;
            }
            KeyCode::Char(character)
                if !control
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !character.is_control() =>
            {
                text.insert(cursor, character);
                cursor += character.len_utf8();
            }
            _ => return Ok(()),
        }
        self.save_view(store, |view| {
            if filter {
                view.filter = text;
            } else {
                view.draft = text;
            }
        })?;
        self.editor_cursor = cursor;
        Ok(())
    }

    pub fn render(&mut self, frame: &mut Frame, connection: &ConnectionInfo) {
        let area = frame.area();
        let chunks = Layout::vertical([
            Constraint::Length(if self.layout.tabs.len() > 1 { 3 } else { 2 }),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);
        let title = self
            .document()
            .map(|document| {
                format!(
                    "{} · revision {}",
                    terminal_safe(&document.title).replace(['\n', '\t'], " "),
                    document.revision
                )
            })
            .unwrap_or_else(|| "Waiting for published content".into());
        let status = format!(
            "{} · generation {} · {}",
            if connection.connected {
                "Connected"
            } else {
                "Disconnected"
            },
            connection.generation,
            terminal_safe(&connection.message).replace(['\n', '\t'], " ")
        );
        let header = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(chunks[0]);
        frame.render_widget(Paragraph::new(format!("Prism: {title}")), header[0]);
        frame.render_widget(Paragraph::new(status), header[1]);
        self.tab_areas.clear();
        self.split_area = Rect::default();
        if self.layout.tabs.len() > 1 {
            let active_index = self
                .layout
                .tabs
                .iter()
                .position(|id| Some(*id) == self.layout.active)
                .unwrap_or(0);
            let visible_count = usize::from(header[2].width / 18).max(1);
            let start = active_index.saturating_sub(visible_count - 1);
            let mut x = header[2].x;
            for (index, id) in self
                .layout
                .tabs
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_count)
            {
                let width = 18.min(header[2].right().saturating_sub(x));
                let area = Rect::new(x, header[2].y, width, header[2].height);
                let Some(view) = self.saved_views.get(id) else {
                    continue;
                };
                let title = self
                    .documents
                    .iter()
                    .find(|document| document.id == view.document_id)
                    .map(|document| terminal_safe(&document.title).replace(['\n', '\t'], " "))
                    .unwrap_or_default();
                let style = if Some(*id) == self.layout.active {
                    Style::default().bg(Color::DarkGray).fg(Color::Yellow)
                } else {
                    Style::default()
                };
                frame.render_widget(
                    Paragraph::new(format!(
                        "{}:{} {}",
                        index + 1,
                        if view.kind == ViewKind::List {
                            "L"
                        } else {
                            "D"
                        },
                        title
                    ))
                    .style(style),
                    area,
                );
                self.tab_areas.push((area, *id));
                x += width;
            }
        }
        if self.show_help {
            self.list_area = Rect::default();
            self.content_area = Rect::default();
            let controls = "j/k or arrows: move focus\nSpace or click: toggle selection\nA: select all visible content\nr: clear stale/current selection\na: accept shared content\nx: archive shared content\nu: undo last content action\nh: hide selected in this view\nH: show all hidden content\n/: edit private filter\nTab / Shift-Tab: switch view\nCtrl-T: new independent view of this document\nCtrl-W: close view, preserve content/draft\nCtrl-R: reopen last closed view\nCtrl-\\: toggle split\nF6 or click other split: focus it\nv: switch list/diagram\nShift-arrows: pan diagram\nAlt-arrows: move focused diagram card\nPgUp/PgDn: scroll details/delivery\n[ / ]: previous/next details page\ne: edit private question draft\nEnter in editor: newline\nCtrl-S or Ctrl-Enter: explicit send\nCtrl-N in editor: delivery mode\nEsc: keep private draft / finish filter\nLeft/Right/Home/End: edit cursor\nBackspace/Delete: edit text\nq / Ctrl-C: quit\n\nAcceptance is feedback, never tool permission.\nShare creates a snapshot for explicit retrieval in the bound CLI; it does not start a turn.\nUnknown delivery is never automatically retried. Sending it again can duplicate the request.";
            frame.render_widget(
                Paragraph::new(controls)
                    .block(Block::bordered().title("Controls · j/k scroll · Esc close"))
                    .wrap(Wrap { trim: false })
                    .scroll((self.detail_scroll, 0)),
                chunks[1],
            );
            frame.render_widget(Paragraph::new("j/k scroll · Esc close · q quit"), chunks[2]);
            return;
        }
        let panes = if chunks[1].width >= 80 {
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .split(chunks[1])
        } else {
            Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)])
                .split(chunks[1])
        };
        if let Some(id) = self.layout.split {
            let split = if panes[0].width >= 72 {
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(panes[0])
            } else {
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(panes[0])
            };
            self.render_content(frame, split[0]);
            self.split_area = split[1];
            self.render_secondary(frame, split[1], id);
        } else {
            self.render_content(frame, panes[0]);
        }
        self.render_details(frame, panes[1]);
        let warning = if self.selection_stale() {
            "STALE selection: r clears; reselect before acting"
        } else {
            &self.message
        };
        let help = match self.input_mode {
            InputMode::Browse => {
                "? controls · q quit · Space select\ne question · a accept · x archive · u undo"
            }
            InputMode::Question => "Ctrl-S send · Ctrl-N mode\nEnter newline · Esc keep draft",
            InputMode::Filter => "Type private filter · Enter/Esc finish · Backspace edit",
        };
        frame.render_widget(
            Paragraph::new(format!("{}\n{help}", terminal_safe(warning)))
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    }

    fn render_content(&mut self, frame: &mut Frame, area: Rect) {
        self.content_area = area;
        if let Some(layout) = self.diagram_layout() {
            self.list_area = Rect::default();
            let selected = self
                .view
                .as_ref()
                .and_then(|view| view.selection.as_ref())
                .map(|selection| selection.ids.iter().copied().collect())
                .unwrap_or_default();
            self.diagram_state
                .render(frame, area, &layout, self.focus, &selected);
        } else {
            self.render_list(frame, area);
        }
    }

    fn render_secondary(&self, frame: &mut Frame, area: Rect, id: Id) {
        let Some(view) = self.saved_views.get(&id) else {
            return;
        };
        let Some(document) = self
            .documents
            .iter()
            .find(|document| document.id == view.document_id)
        else {
            return;
        };
        let presentation = self
            .layout
            .presentation
            .get(&id)
            .cloned()
            .unwrap_or_default();
        if view.kind == ViewKind::Diagram {
            let mut state = diagram::DiagramState {
                pan_x: presentation.pan_x,
                pan_y: presentation.pan_y,
            };
            let selected = view
                .selection
                .as_ref()
                .map(|selection| selection.ids.iter().copied().collect())
                .unwrap_or_default();
            state.render(
                frame,
                area,
                &diagram::DiagramLayout::new(document, view),
                presentation.focus,
                &selected,
            );
            if view
                .selection
                .as_ref()
                .is_some_and(|selection| selection.revision != document.revision)
            {
                frame.render_widget(
                    Paragraph::new("STALE selection · F6 focus")
                        .style(Style::default().fg(Color::Yellow)),
                    Rect::new(area.x, area.y, area.width, area.height.min(1)),
                );
            }
            return;
        }
        let rows = list::rows(document, view);
        let selected = view.selection.as_ref();
        let mut state = ListState::default()
            .with_offset(presentation.list_offset)
            .with_selected(
                rows.iter()
                    .position(|row| Some(row.id) == presentation.focus),
            );
        let items = rows.iter().map(|row| {
            ListItem::new(format!(
                "{} {} {}",
                if selected.is_some_and(|selection| selection.ids.contains(&row.id)) {
                    "[x]"
                } else {
                    "[ ]"
                },
                row.label,
                if row.stale_acceptance {
                    "accepted stale"
                } else if row.accepted {
                    "accepted"
                } else {
                    ""
                }
            ))
        });
        let stale = selected.is_some_and(|selection| selection.revision != document.revision);
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::bordered().title(format!(
                    "{} · {} · F6 focus{}",
                    terminal_safe(&document.title).replace(['\n', '\t'], " "),
                    document.revision,
                    if stale { " · STALE selection" } else { "" }
                )))
                .highlight_style(Style::default().bg(Color::DarkGray)),
            area,
            &mut state,
        );
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.rows();
        let selection = self.view.as_ref().and_then(|view| view.selection.as_ref());
        let filter = self
            .view
            .as_ref()
            .map(|view| terminal_safe(&view.filter))
            .unwrap_or_default();
        let count = selection.map_or(0, |selection| selection.ids.len());
        let block =
            Block::bordered().title(format!("Content · {count} selected · filter: {filter}"));
        self.list_area = block.inner(area);
        let items = rows.iter().map(|row| {
            let selected = selection.is_some_and(|selection| selection.ids.contains(&row.id));
            let acceptance = if row.stale_acceptance {
                "accepted stale"
            } else if row.accepted {
                "accepted"
            } else {
                ""
            };
            ListItem::new(format!(
                "{} {} {acceptance}",
                if selected { "[x]" } else { "[ ]" },
                row.label
            ))
        });
        frame.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(Style::default().bg(Color::DarkGray))
                .highlight_symbol("> "),
            area,
            &mut self.list_state,
        );
    }

    fn render_details(&self, frame: &mut Frame, area: Rect) {
        if self.input_mode == InputMode::Question || self.input_mode == InputMode::Filter {
            let text = self
                .view
                .as_ref()
                .map(|view| {
                    if self.input_mode == InputMode::Filter {
                        view.filter.as_str()
                    } else {
                        view.draft.as_str()
                    }
                })
                .unwrap_or("");
            let cursor = text.floor_char_boundary(self.editor_cursor.min(text.len()));
            let before = terminal_safe(&text[..cursor]).replace('\t', "    ");
            let after = terminal_safe(&text[cursor..]).replace('\t', "    ");
            // A styled cursor marker avoids splitting UTF-8 or guessing terminal cell widths.
            let mut lines: Vec<Line> = before
                .split('\n')
                .map(|line| Line::raw(line.to_owned()))
                .collect();
            let current = lines.last_mut().expect("split always yields a line");
            current
                .spans
                .push(Span::styled("▏", Style::default().fg(Color::Yellow)));
            let mut tail = after.split('\n');
            current
                .spans
                .push(Span::raw(tail.next().unwrap_or("").to_owned()));
            lines.extend(tail.map(|line| Line::raw(line.to_owned())));
            let title = if self.input_mode == InputMode::Filter {
                "Private filter".into()
            } else {
                format!(
                    "Private question · {:?} · {} selected · generation {}",
                    self.delivery_mode,
                    self.view
                        .as_ref()
                        .and_then(|view| view.selection.as_ref())
                        .map_or(0, |selection| selection.ids.len()),
                    self.editor_generation.unwrap_or(0)
                )
            };
            let block = Block::bordered().title(title);
            let inner = block.inner(area);
            let cursor_row = before.split('\n').count().saturating_sub(1);
            let cursor_column = Line::raw(before.rsplit('\n').next().unwrap_or("")).width();
            let scroll_y = cursor_row
                .saturating_sub(usize::from(inner.height.saturating_sub(1)))
                .min(usize::from(u16::MAX)) as u16;
            let scroll_x = cursor_column
                .saturating_sub(usize::from(inner.width.saturating_sub(1)))
                .min(usize::from(u16::MAX)) as u16;
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .scroll((scroll_y, scroll_x)),
                area,
            );
            return;
        }
        let details = self.details_text();
        let pages = details
            .chars()
            .count()
            .div_ceil(DETAIL_PAGE_CHARACTERS)
            .max(1);
        let page = self.detail_page.min(pages - 1);
        // Bound wrapped lines before handing the page to Ratatui's u16 scroll offset.
        let text: String = details
            .chars()
            .skip(page * DETAIL_PAGE_CHARACTERS)
            .take(DETAIL_PAGE_CHARACTERS)
            .collect();
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::bordered().title(format!(
                    "Details / delivery · page {}/{} · [/] · PgUp/PgDn",
                    page + 1,
                    pages
                )))
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0)),
            area,
        );
    }

    fn details_text(&self) -> String {
        let mut details = self.rows().into_iter().find(|row| Some(row.id) == self.focus).map(|row| row.details).unwrap_or_else(|| "Publish content from the bound CLI, or clear the filter/show hidden entries with H.".into());
        if let Some(selection) = self.view.as_ref().and_then(|view| view.selection.as_ref()) {
            details.push_str(&format!(
                "\n\nSelection revision {}: {} IDs",
                selection.revision,
                selection.ids.len()
            ));
        }
        details.push_str("\n\nQuestions (never automatically retried):");
        for question in &self.questions {
            details.push_str(&format!(
                "\n{} · {:?} · generation {}\n{}\n{}",
                question.request_id,
                question.mode,
                question.generation,
                terminal_safe(&question.text),
                outcome_label(question)
            ));
        }
        details
    }
}

fn outcome_label(question: &Question) -> String {
    match &question.outcome {
        DeliveryOutcome::Pending if question.mode == DeliveryMode::Share => {
            "Shared for explicit CLI retrieval; no native turn started".into()
        }
        DeliveryOutcome::Pending => "Pending bridge dispatch".into(),
        DeliveryOutcome::Submitted => "Submitted; start not confirmed".into(),
        DeliveryOutcome::Queued { reason } => format!(
            "Queued; start not confirmed: {}",
            terminal_safe(reason.as_deref().unwrap_or(""))
        ),
        DeliveryOutcome::Started { turn_id } => format!("Started: {}", terminal_safe(turn_id)),
        DeliveryOutcome::Failed { reason } => format!("Failed: {}", terminal_safe(reason)),
        DeliveryOutcome::Refused { reason } => format!("Refused: {}", terminal_safe(reason)),
        DeliveryOutcome::Dropped { reason } => format!("Dropped: {}", terminal_safe(reason)),
        DeliveryOutcome::Unknown { reason } => format!(
            "Unknown: {}. A new explicit send may duplicate the request.",
            terminal_safe(reason)
        ),
    }
}

struct TerminalGuard {
    signals: Vec<signal_hook::SigId>,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

pub fn run(
    mut store: Store,
    workspace: Workspace,
    mut status: impl FnMut() -> ConnectionInfo,
) -> Result<()> {
    let mut state = UiState::new(&mut store, workspace)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    let mut guard = TerminalGuard {
        signals: Vec::new(),
    };
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        guard.signals.push(signal_hook::flag::register(
            signal,
            Arc::clone(&interrupted),
        )?);
    }
    let mut terminal = ratatui::try_init()?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |information| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        previous_hook(information);
    }));
    execute!(io::stdout(), EnableMouseCapture)?;
    let mut refreshed = Instant::now();
    while !interrupted.load(Ordering::Relaxed) {
        let connection = status();
        if refreshed.elapsed() >= REFRESH_INTERVAL {
            if let Err(error) = state.refresh(&mut store) {
                state.message = format!("Refresh error: {error}");
            }
            refreshed = Instant::now();
        }
        terminal.draw(|frame| state.render(frame, &connection))?;
        if event::poll(EVENT_WAIT)? {
            let event = event::read()?;
            // Refresh the binding immediately before explicit actions, not just before drawing.
            if state.handle_event(&mut store, event, &status()) {
                break;
            }
        }
    }
    drop(guard);
    Ok(())
}
