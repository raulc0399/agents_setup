pub mod diagram;
pub mod graph;
pub mod list;
pub mod theme;
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
use ratatui::layout::{Constraint, Layout, Position as CellPosition, Rect};
use ratatui::style::{Color, Modifier, Style};
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
const DETAIL_PAGE_LINES: usize = 4_096;
const TAB_WIDTH: u16 = 18;
const HELP_TWO_COLUMN_WIDTH: u16 = 80;
const CARD_INDENT: &str = "    ";
const WIDE_WIDTH: u16 = 120;
const EDITOR_HEIGHT: u16 = 7;
const FILTER_HEIGHT: u16 = 3;
const EXPANDED_RESERVED_LINES: u16 = 5;
const EXPANDED_MIN_LINES: u16 = 3;
const EXPANDED_MAX_LINES: u16 = 30;
const EMPTY_DETAILS: &str =
    "Publish content from the bound CLI, or clear the filter and press H to show hidden items.";
const DIAGRAM_PAN_STEP: i32 = 4;
const DIAGRAM_MOVE_STEP: i32 = 2;
const GRAPH_PAN_CELLS: i32 = 3;
const GRAPH_PANEL_MIN_HEIGHT: u16 = 6;
const GRAPH_PANEL_MAX_HEIGHT: u16 = 12;
const EXPORT_DIRECTORY: &str = ".prism/export";

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
    show_help: bool,
    pub layout: views::ViewLayout,
    saved_views: BTreeMap<Id, ViewState>,
    diagram_state: diagram::DiagramState,
    content_area: Rect,
    split_area: Rect,
    tab_areas: Vec<(Rect, Id)>,
    card_areas: Vec<(Rect, Id)>,
    action_areas: Vec<(Rect, CardAction)>,
    /// Text columns of the expanded card; a drag inside selects text for `ask`.
    expanded_area: Option<Rect>,
    drag_start: Option<CellPosition>,
    pending_click: Option<Id>,
    text_selection: Option<(CellPosition, CellPosition)>,
    pub selected_text: String,
    /// Image-protocol diagram; None keeps the text diagram.
    graph: Option<graph::GraphView>,
    graph_area: Rect,
    /// Where this frame and the previous one drew an image; a change needs a full repaint
    /// because the terminal keeps image pixels under cells the diff leaves untouched.
    image_area: Option<Rect>,
    previous_image_area: Option<Rect>,
    force_redraw: bool,
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
            card_areas: Vec::new(),
            action_areas: Vec::new(),
            expanded_area: None,
            drag_start: None,
            pending_click: None,
            text_selection: None,
            selected_text: String::new(),
            graph: None,
            graph_area: Rect::default(),
            image_area: None,
            previous_image_area: None,
            force_redraw: false,
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
        let id = self
            .layout
            .closed
            .pop()
            .ok_or_else(|| anyhow::anyhow!("No closed view to reopen"))?;
        if self.layout.tabs.len() >= views::MAX_OPEN_VIEWS {
            // At capacity the reopened view takes the active slot. The active view goes to the
            // bottom of the reopen stack so repeated Ctrl-R cycles through every closed view.
            if let Some(active) = self.layout.active {
                self.layout.tabs.retain(|tab| *tab != active);
                self.layout.closed.insert(0, active);
                if self.layout.split == Some(active) {
                    self.layout.split = None;
                }
            }
        }
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

    pub fn set_graph(&mut self, graph: Option<graph::GraphView>) {
        self.graph = graph;
    }

    /// True once after a frame whose image area differs from the previous frame's, or after
    /// Ctrl-L: the caller clears the terminal and draws again.
    pub fn take_redraw(&mut self) -> bool {
        let moved =
            self.previous_image_area.is_some() && self.previous_image_area != self.image_area;
        self.previous_image_area = self.image_area;
        std::mem::take(&mut self.force_redraw) || moved
    }

    fn graph_active(&self) -> bool {
        self.graph.is_some()
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.kind == ViewKind::Diagram)
    }

    fn focus_rect(&self) -> Option<graph::WorldRect> {
        self.graph.as_ref()?.layout().rect_of(self.focus?)
    }

    /// The text diagram; None while the graphic diagram owns the view.
    fn diagram_layout(&self) -> Option<diagram::DiagramLayout> {
        let view = self.view.as_ref()?;
        (view.kind == ViewKind::Diagram && self.graph.is_none()).then(|| {
            diagram::DiagramLayout::new(self.document().expect("view document exists"), view)
        })
    }

    fn visible_ids(&self) -> Vec<Id> {
        if self.graph_active() {
            return self.graph.as_ref().expect("graph active").layout().ids();
        }
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
        self.clear_text_selection();
        if self.graph_active() {
            let ids = self.visible_ids();
            if ids.is_empty() {
                return;
            }
            let index = ids
                .iter()
                .position(|id| Some(*id) == self.focus)
                .unwrap_or(0)
                .saturating_add_signed(delta)
                .min(ids.len() - 1);
            self.focus = Some(ids[index]);
            if let (Some(rect), Some(graph)) = (self.focus_rect(), self.graph.as_mut()) {
                graph.reveal(rect);
            }
            self.detail_scroll = 0;
            self.detail_page = 0;
            return;
        }
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

    /// Actions on an empty selection target the focused item, like the pills on its card.
    fn selection_or_focus(&mut self, store: &mut Store) -> Result<Selection> {
        let has_selection = self
            .view
            .as_ref()
            .and_then(|view| view.selection.as_ref())
            .is_some_and(|selection| !selection.ids.is_empty());
        if !has_selection && let Some(id) = self.focus {
            let document = self
                .document()
                .ok_or_else(|| anyhow::anyhow!("No document"))?;
            let selection = Selection {
                document_id: document.id,
                revision: document.revision,
                ids: vec![id],
            };
            self.save_view(store, |view| view.selection = Some(selection))?;
        }
        self.selection()
    }

    fn focus_card(&mut self, id: Id) {
        self.focus = Some(id);
        self.list_state
            .select(self.rows().iter().position(|row| row.id == id));
        self.detail_scroll = 0;
        self.detail_page = 0;
    }

    fn hide_selection(&mut self, store: &mut Store) -> Result<()> {
        let ids = self.selection_or_focus(store)?.ids;
        self.save_view(store, |view| {
            view.hidden.extend(ids);
            view.selection = None;
        })
    }

    fn open_editor(&mut self, store: &mut Store, connection: &ConnectionInfo) -> Result<()> {
        self.selection_or_focus(store)?;
        self.input_mode = InputMode::Question;
        self.editor_cursor = self.view.as_ref().map_or(0, |view| view.draft.len());
        self.editor_generation = Some(connection.generation);
        Ok(())
    }

    fn run_card_action(
        &mut self,
        store: &mut Store,
        action: CardAction,
        connection: &ConnectionInfo,
    ) -> Result<()> {
        match action {
            CardAction::Ask => self.ask(store, connection),
            CardAction::Hide => self.hide_selection(store),
            CardAction::Drop => self.apply(store, true),
        }
    }

    /// Shares the focused item, or the text dragged inside it, as a quoted reference. The
    /// question itself is typed in the CLI once the mod places this text in its prompt box.
    fn ask(&mut self, store: &mut Store, connection: &ConnectionInfo) -> Result<()> {
        if connection.generation == 0 {
            bail!("Binding unavailable; attach the CLI before asking");
        }
        let rows = self.rows();
        let (index, row) = rows
            .iter()
            .enumerate()
            .find(|(_, row)| Some(row.id) == self.focus)
            .ok_or_else(|| anyhow::anyhow!("Focus an item first"))?;
        let quoted = if self.selected_text.trim().is_empty() {
            row.details.clone()
        } else {
            self.selected_text.clone()
        };
        let mut text = format!("{ASK_REFERENCE_PREFIX} {}: {}", index + 1, row.title);
        for line in quoted.lines() {
            text.push_str(&format!("\n> {line}"));
        }
        let selection = self.selection_or_focus(store)?;
        let question = store.prepare_question(
            self.workspace.id,
            &PrepareQuestion {
                request_id: Id::new_v4(),
                generation: connection.generation,
                selection,
                text,
                mode: DeliveryMode::Share,
            },
        )?;
        self.questions.push(question);
        self.clear_text_selection();
        self.message = "Shared for the CLI prompt box; type your question there.".into();
        Ok(())
    }

    fn export_svg(&mut self) -> Result<()> {
        let (Some(graph), Some(view)) = (self.graph.as_ref(), self.view.as_ref()) else {
            bail!("Open the diagram view to export it");
        };
        let document = self
            .documents
            .iter()
            .find(|document| document.id == view.document_id)
            .ok_or_else(|| anyhow::anyhow!("No document"))?;
        let directory = std::path::Path::new(&self.workspace.key.project).join(EXPORT_DIRECTORY);
        let path = graph.export_svg(document, view, &directory)?;
        self.message = format!("Exported {}", path.display());
        Ok(())
    }

    fn clear_text_selection(&mut self) {
        self.drag_start = None;
        self.pending_click = None;
        self.text_selection = None;
        self.selected_text.clear();
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
        let selection = self.selection_or_focus(store)?;
        let action = if archive {
            ContentAction::Archive { ids: selection.ids }
        } else {
            ContentAction::Accept { ids: selection.ids }
        };
        store.apply(
            self.workspace.id,
            selection.document_id,
            selection.revision,
            &action,
        )?;
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
        // Persisted records survive restarts and cover archives made through the agent tool.
        store.undo_latest(self.workspace.id, id, revision)?;
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
            let position = CellPosition::new(mouse.column, mouse.row);
            if self.graph_active() && self.graph_area.contains(position) {
                let control = mouse.modifiers.contains(KeyModifiers::CONTROL);
                let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
                let anchor = self.focus_rect();
                let hit = match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => self
                        .graph
                        .as_ref()
                        .and_then(|graph| graph.hit(self.graph_area, mouse.column, mouse.row)),
                    _ => None,
                };
                if let Some(graph) = self.graph.as_mut() {
                    let step = graph.wheel_zoom_step();
                    match mouse.kind {
                        MouseEventKind::ScrollDown if control => graph.zoom_by(1.0 / step, anchor),
                        MouseEventKind::ScrollUp if control => graph.zoom_by(step, anchor),
                        MouseEventKind::ScrollDown if shift => {
                            graph.pan_by_cells(GRAPH_PAN_CELLS, 0)
                        }
                        MouseEventKind::ScrollUp if shift => {
                            graph.pan_by_cells(-GRAPH_PAN_CELLS, 0)
                        }
                        MouseEventKind::ScrollDown => graph.pan_by_cells(0, GRAPH_PAN_CELLS),
                        MouseEventKind::ScrollUp => graph.pan_by_cells(0, -GRAPH_PAN_CELLS),
                        MouseEventKind::ScrollRight => graph.pan_by_cells(GRAPH_PAN_CELLS, 0),
                        MouseEventKind::ScrollLeft => graph.pan_by_cells(-GRAPH_PAN_CELLS, 0),
                        _ => {}
                    }
                }
                if let Some(id) = hit {
                    self.clear_text_selection();
                    self.focus = Some(id);
                    self.detail_scroll = 0;
                    self.detail_page = 0;
                    self.toggle_selection(store)?;
                }
                return Ok(false);
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
            if let MouseEventKind::Up(MouseButton::Left) = mouse.kind {
                // A click toggles on release so a drag never toggles the card it started on.
                if let Some(id) = self.pending_click.take()
                    && self.text_selection.is_none()
                {
                    self.focus_card(id);
                    self.toggle_selection(store)?;
                }
                self.drag_start = None;
                return Ok(false);
            }
            if let MouseEventKind::Drag(MouseButton::Left) = mouse.kind {
                if let (Some(start), Some(area)) = (self.drag_start, self.expanded_area)
                    && area.contains(start)
                {
                    let end = CellPosition::new(
                        position.x.clamp(area.x, area.right().saturating_sub(1)),
                        position.y.clamp(area.y, area.bottom().saturating_sub(1)),
                    );
                    self.text_selection = Some(if (end.y, end.x) < (start.y, start.x) {
                        (end, start)
                    } else {
                        (start, end)
                    });
                }
                return Ok(false);
            }
            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                if let Some((_, action)) = self
                    .action_areas
                    .iter()
                    .find(|(area, _)| area.contains(position))
                {
                    self.run_card_action(store, *action, connection)?;
                } else if let Some((_, id)) = self
                    .card_areas
                    .iter()
                    .find(|(area, _)| area.contains(position))
                {
                    self.text_selection = None;
                    self.selected_text.clear();
                    self.drag_start = Some(position);
                    self.pending_click = Some(*id);
                } else if self
                    .expanded_area
                    .is_some_and(|area| area.contains(position))
                {
                    // The diagram panel: a drag selects text, a click does nothing else.
                    self.text_selection = None;
                    self.selected_text.clear();
                    self.drag_start = Some(position);
                }
                return Ok(false);
            }
            if self.list_area.contains(position) {
                match mouse.kind {
                    MouseEventKind::ScrollDown => self.move_focus(1),
                    MouseEventKind::ScrollUp => self.move_focus(-1),
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
                KeyCode::Char('l') => {
                    self.force_redraw = true;
                    if let Some(graph) = self.graph.as_mut() {
                        graph.invalidate();
                    }
                }
                // Legacy terminal key mode encodes Ctrl-\ as 0x1C, which crossterm reports as Ctrl-4.
                KeyCode::Char('\\') | KeyCode::Char('4') => self.toggle_split(store)?,
                _ => return Ok(false),
            }
            return Ok(false);
        }
        if self.graph_active() {
            let anchor = self.focus_rect();
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
            let pan = match key.code {
                KeyCode::Left if shift => Some((-GRAPH_PAN_CELLS, 0)),
                KeyCode::Right if shift => Some((GRAPH_PAN_CELLS, 0)),
                KeyCode::Up if shift => Some((0, -GRAPH_PAN_CELLS)),
                KeyCode::Down if shift => Some((0, GRAPH_PAN_CELLS)),
                _ => None,
            };
            if let Some(graph) = self.graph.as_mut() {
                match key.code {
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        graph.zoom_by(graph::ZOOM_STEP, anchor);
                        return Ok(false);
                    }
                    KeyCode::Char('-') => {
                        graph.zoom_by(1.0 / graph::ZOOM_STEP, anchor);
                        return Ok(false);
                    }
                    KeyCode::Char('0') => {
                        graph.reset_zoom();
                        return Ok(false);
                    }
                    _ => {}
                }
                if let Some((dx, dy)) = pan {
                    graph.pan_by_cells(dx, dy);
                    return Ok(false);
                }
            }
            if key.code == KeyCode::Char('s') {
                self.export_svg()?;
                return Ok(false);
            }
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
            KeyCode::Char('h') => self.hide_selection(store)?,
            KeyCode::Char('H') => self.save_view(store, |view| view.hidden.clear())?,
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Filter;
                self.editor_cursor = self.view.as_ref().map_or(0, |view| view.filter.len());
            }
            KeyCode::Char('e') => self.open_editor(store, connection)?,
            KeyCode::Enter => self.ask(store, connection)?,
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
            KeyCode::PageDown => {
                self.clear_text_selection();
                self.detail_scroll = self.detail_scroll.saturating_add(5);
            }
            KeyCode::PageUp => {
                self.clear_text_selection();
                self.detail_scroll = self.detail_scroll.saturating_sub(5);
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                self.clear_text_selection();
                let pages = self.detail_page_count();
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
        self.image_area = None;
        let area = frame.area();
        let tabs_height = u16::from(self.layout.tabs.len() > 1);
        let editor_height = match self.input_mode {
            InputMode::Browse => 0,
            InputMode::Question => EDITOR_HEIGHT,
            InputMode::Filter => FILTER_HEIGHT,
        };
        let chunks = Layout::vertical([
            Constraint::Length(1 + tabs_height),
            Constraint::Min(1),
            Constraint::Length(editor_height),
            Constraint::Length(2),
        ])
        .split(area);
        let header = Layout::vertical([Constraint::Length(1), Constraint::Length(tabs_height)])
            .split(chunks[0]);
        self.render_header(frame, header[0], connection);
        self.render_tabs(frame, header[1]);
        if self.show_help {
            self.list_area = Rect::default();
            self.content_area = Rect::default();
            self.card_areas.clear();
            self.action_areas.clear();
            render_help(frame, chunks[1], self.detail_scroll);
            frame.render_widget(
                Paragraph::new("j/k scroll · Esc close · q quit").style(theme::hint()),
                chunks[3],
            );
            return;
        }
        // A pane beside the CLI is narrow: one column, the focused card expands in place.
        // Only a wide terminal gets a separate details column.
        let (column, details) = if chunks[1].width >= WIDE_WIDTH {
            let panes =
                Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                    .split(chunks[1]);
            (panes[0], Some(panes[1]))
        } else {
            (chunks[1], None)
        };
        let primary = if let Some(id) = self.layout.split {
            let split = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(column);
            self.split_area = split[1];
            self.render_secondary(frame, split[1], id);
            split[0]
        } else {
            column
        };
        self.render_content(frame, primary, details.is_none());
        if let Some(details) = details {
            self.render_details(frame, details);
        }
        if editor_height > 0 {
            self.render_editor(frame, chunks[2]);
        }
        self.render_footer(frame, chunks[3]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, connection: &ConnectionInfo) {
        let status = if connection.connected {
            Line::styled("● Connected", theme::colored(theme::CONNECTED))
        } else {
            Line::styled(
                format!(
                    "● Disconnected · {}",
                    list::single_line(&connection.message)
                ),
                theme::colored(theme::ERROR),
            )
        };
        let status_width = u16::try_from(status.width())
            .unwrap_or(u16::MAX)
            .min(area.width);
        let title_area = Rect::new(
            area.x,
            area.y,
            area.width.saturating_sub(status_width + 1),
            area.height,
        );
        let mut title = vec![Span::styled("Prism ", theme::bold().fg(theme::PRIMARY))];
        match self.document() {
            Some(document) => {
                title.push(Span::styled(
                    list::single_line(&document.title),
                    theme::bold(),
                ));
                title.push(Span::styled(
                    format!(" · revision {}", document.revision),
                    theme::hint(),
                ));
            }
            None => title.push(Span::styled("Waiting for published content", theme::hint())),
        }
        frame.render_widget(Paragraph::new(Line::from(title)), title_area);
        frame.render_widget(Paragraph::new(status).right_aligned(), area);
    }

    fn render_tabs(&mut self, frame: &mut Frame, area: Rect) {
        self.tab_areas.clear();
        self.split_area = Rect::default();
        if area.height == 0 || self.layout.tabs.len() <= 1 {
            return;
        }
        let active_index = self
            .layout
            .tabs
            .iter()
            .position(|id| Some(*id) == self.layout.active)
            .unwrap_or(0);
        let visible_count = usize::from(area.width / TAB_WIDTH).max(1);
        let start = active_index.saturating_sub(visible_count - 1);
        let mut x = area.x;
        for (index, id) in self
            .layout
            .tabs
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_count)
        {
            let width = TAB_WIDTH.min(area.right().saturating_sub(x));
            if width < 2 {
                break;
            }
            let Some(view) = self.saved_views.get(id) else {
                continue;
            };
            let title = self
                .documents
                .iter()
                .find(|document| document.id == view.document_id)
                .map(|document| list::single_line(&document.title))
                .unwrap_or_default();
            let kind = if view.kind == ViewKind::List {
                "L"
            } else {
                "D"
            };
            let label = clip(
                &format!("{} {kind} {title}", index + 1),
                usize::from(width) - 2,
            );
            let style = if Some(*id) == self.layout.active {
                theme::bold().fg(theme::PRIMARY)
            } else {
                theme::hint()
            };
            let tab_area = Rect::new(x, area.y, width, area.height);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!(" {label:<pad$}", pad = usize::from(width) - 2),
                        style,
                    ),
                    Span::styled("│", theme::hint()),
                ])),
                tab_area,
            );
            self.tab_areas.push((tab_area, *id));
            x += width;
        }
    }

    fn render_content(&mut self, frame: &mut Frame, area: Rect, expand: bool) {
        self.content_area = area;
        if self.graph_active() {
            self.render_graph(frame, area);
            return;
        }
        if let Some(layout) = self.diagram_layout() {
            self.list_area = Rect::default();
            self.card_areas.clear();
            self.action_areas.clear();
            let selected = self
                .view
                .as_ref()
                .and_then(|view| view.selection.as_ref())
                .map(|selection| selection.ids.iter().copied().collect())
                .unwrap_or_default();
            self.diagram_state
                .render(frame, area, &layout, self.focus, &selected);
        } else {
            self.render_list(frame, area, expand);
        }
    }

    /// Image diagram on top, a text panel for the focused box, arrow or gate below it.
    fn render_graph(&mut self, frame: &mut Frame, area: Rect) {
        self.list_area = Rect::default();
        self.card_areas.clear();
        self.action_areas.clear();
        self.expanded_area = None;
        let focused = self
            .rows()
            .into_iter()
            .find(|row| Some(row.id) == self.focus);
        let panel_height = if focused.is_some() {
            (area.height / 3)
                .clamp(GRAPH_PANEL_MIN_HEIGHT, GRAPH_PANEL_MAX_HEIGHT)
                .min(area.height.saturating_sub(4))
        } else {
            0
        };
        let chunks =
            Layout::vertical([Constraint::Min(1), Constraint::Length(panel_height)]).split(area);
        let block = Block::bordered();
        let inner = block.inner(chunks[0]);
        self.graph_area = inner;
        let focus = self.focus;
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let Some(document) = self
            .documents
            .iter()
            .find(|document| document.id == view.document_id)
        else {
            return;
        };
        let Some(graph) = self.graph.as_mut() else {
            return;
        };
        let selected: BTreeSet<Id> = view
            .selection
            .as_ref()
            .map(|selection| selection.ids.iter().copied().collect())
            .unwrap_or_default();
        graph.prepare(document, view, (inner.width, inner.height));
        let title = format!(
            "Diagram · {} boxes · zoom {:.1}x · + - 0 · s export",
            graph.layout().node_count(),
            graph.zoom
        );
        frame.render_widget(block.title(title), chunks[0]);
        if graph.layout().is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "Nothing to draw: publish elements, or press H to show hidden items",
                )
                .style(theme::hint())
                .wrap(Wrap { trim: false }),
                inner,
            );
        } else {
            graph.render(frame, inner, document, view, focus, &selected);
            self.image_area = Some(inner);
        }
        if let Some(row) = focused
            && panel_height > 0
        {
            self.render_graph_panel(frame, chunks[1], &row);
        }
    }

    fn render_graph_panel(&mut self, frame: &mut Frame, area: Rect, row: &list::Row) {
        let block = Block::bordered().title(Line::from(vec![
            Span::styled(row.title.clone(), theme::bold()),
            Span::styled(format!(" · {}", row.kind), theme::colored(row.color)),
        ]));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 2 {
            return;
        }
        let text_area = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
        frame.render_widget(
            Paragraph::new(self.details_lines(row))
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0)),
            text_area,
        );
        let pills_y = inner.bottom() - 1;
        let mut spans = Vec::new();
        let mut x = inner.x;
        for (action, label) in CARD_ACTIONS {
            let style = if action == CardAction::Ask {
                theme::bold().fg(theme::PRIMARY)
            } else {
                theme::bold()
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            let width = label.len() as u16;
            if x + width <= inner.right() {
                self.action_areas
                    .push((Rect::new(x, pills_y, width, 1), action));
            }
            x += width + 1;
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, pills_y, inner.width, 1),
        );
        self.expanded_area = Some(text_area);
        self.paint_text_selection(frame);
    }

    fn latest_question(&self, id: Id) -> Option<&Question> {
        self.questions
            .iter()
            .find(|question| question.snapshot.selection.ids.contains(&id))
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
        let stale = view
            .selection
            .as_ref()
            .is_some_and(|selection| selection.revision != document.revision);
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
            if stale {
                frame.render_widget(
                    Paragraph::new("STALE selection · F6 focus")
                        .style(theme::colored(theme::CHANGED)),
                    Rect::new(area.x, area.y, area.width, area.height.min(1)),
                );
            }
            return;
        }
        let rows = list::rows(document, view);
        let focused = rows
            .iter()
            .position(|row| Some(row.id) == presentation.focus);
        let block = Block::bordered().title(format!(
            "{}{} · revision {} · F6 focus",
            if stale { "STALE selection · " } else { "" },
            list::single_line(&document.title),
            document.revision
        ));
        let width = usize::from(block.inner(area).width);
        let items = rows.iter().enumerate().map(|(index, row)| {
            ListItem::new(card_lines(
                row,
                index + 1,
                Some(index) == focused,
                self.latest_question(row.id),
                width,
                false,
                None,
            ))
        });
        let mut state = ListState::default()
            .with_offset(presentation.list_offset)
            .with_selected(focused);
        frame.render_stateful_widget(List::new(items).block(block), area, &mut state);
    }

    fn render_list(&mut self, frame: &mut Frame, area: Rect, expand: bool) {
        let rows = self.rows();
        let view = self.view.as_ref();
        let selected: BTreeSet<Id> = view
            .and_then(|view| view.selection.as_ref())
            .map(|selection| selection.ids.iter().copied().collect())
            .unwrap_or_default();
        let filter = view
            .map(|view| list::single_line(&view.filter))
            .unwrap_or_default();
        let mut title = format!("Content · {} items", rows.len());
        if !selected.is_empty() {
            title.push_str(&format!(" · {} selected", selected.len()));
        }
        if !filter.is_empty() {
            title.push_str(&format!(" · filter: {filter}"));
        }
        let block = Block::bordered().title(title);
        let inner = block.inner(area);
        self.list_area = inner;
        let focused = self.list_state.selected();
        let width = usize::from(inner.width);
        let expanded = if expand {
            let window = usize::from(
                inner
                    .height
                    .saturating_sub(EXPANDED_RESERVED_LINES)
                    .clamp(EXPANDED_MIN_LINES, EXPANDED_MAX_LINES),
            );
            focused
                .and_then(|index| rows.get(index))
                .map(|row| self.expanded_window(row, width, window))
        } else {
            None
        };
        let cards: Vec<Vec<Line>> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let is_focused = Some(index) == focused;
                card_lines(
                    row,
                    index + 1,
                    is_focused,
                    self.latest_question(row.id),
                    width,
                    true,
                    expanded.as_deref().filter(|_| is_focused),
                )
            })
            .collect();
        let heights: Vec<u16> = cards
            .iter()
            .map(|lines| u16::try_from(lines.len()).unwrap_or(u16::MAX))
            .collect();
        frame.render_stateful_widget(
            List::new(cards.into_iter().map(ListItem::new)).block(block),
            area,
            &mut self.list_state,
        );
        self.record_card_areas(inner, &rows, &heights, focused);
        self.expanded_area = expanded.and(focused).and_then(|index| {
            let (area, _) = self
                .card_areas
                .iter()
                .find(|(_, id)| Some(*id) == rows[index].id.into())?;
            let text_x = area.x + CARD_INDENT.len() as u16;
            (area.height > 2 && area.width as usize > CARD_INDENT.len()).then(|| {
                Rect::new(
                    text_x,
                    area.y + 1,
                    area.width - CARD_INDENT.len() as u16,
                    area.height - 2,
                )
            })
        });
        self.paint_text_selection(frame);
    }

    /// Highlights the dragged cells and captures their text exactly as displayed.
    fn paint_text_selection(&mut self, frame: &mut Frame) {
        let (Some((start, end)), Some(area)) = (self.text_selection, self.expanded_area) else {
            return;
        };
        let buffer = frame.buffer_mut();
        let mut text = String::new();
        for y in start.y.max(area.y)..=end.y.min(area.bottom().saturating_sub(1)) {
            let from = if y == start.y { start.x } else { area.x };
            let to = if y == end.y {
                end.x
            } else {
                area.right().saturating_sub(1)
            };
            let mut line = String::new();
            for x in from.max(area.x)..=to.min(area.right().saturating_sub(1)) {
                let cell = &mut buffer[(x, y)];
                line.push_str(cell.symbol());
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(line.trim_end());
        }
        self.selected_text = text;
    }

    /// The visible slice of the focused card's details: the current page, wrapped to the card,
    /// scrolled by PgUp/PgDn, with a marker when more follows.
    fn expanded_window(&self, row: &list::Row, width: usize, window: usize) -> Vec<Line<'static>> {
        let lines = self.details_lines(row);
        let pages = lines.len().div_ceil(DETAIL_PAGE_LINES).max(1);
        let page = self.detail_page.min(pages - 1);
        let text_width = width.saturating_sub(CARD_INDENT.len());
        let prefix = [gutter(true), Span::raw("  ")];
        let scroll = usize::from(self.detail_scroll);
        let needed = scroll + window + 1;
        let mut wrapped = Vec::new();
        for line in lines
            .iter()
            .skip(page * DETAIL_PAGE_LINES)
            .take(DETAIL_PAGE_LINES)
        {
            wrapped.extend(wrap_line(line, text_width, &prefix));
            if wrapped.len() >= needed {
                break;
            }
        }
        let more = wrapped.len() > scroll + window;
        let mut visible: Vec<Line> = wrapped
            .into_iter()
            .skip(scroll)
            .take(if more { window - 1 } else { window })
            .collect();
        if more || pages > 1 {
            let mut marker = String::from("  ");
            if more {
                marker.push_str("↓ more · PgDn");
            }
            if pages > 1 {
                marker.push_str(&format!(" · page {}/{} · [ ]", page + 1, pages));
            }
            visible.push(Line::from(vec![
                gutter(true),
                Span::styled(marker, theme::hint()),
            ]));
        }
        visible
    }

    fn record_card_areas(
        &mut self,
        inner: Rect,
        rows: &[list::Row],
        heights: &[u16],
        focused: Option<usize>,
    ) {
        self.card_areas.clear();
        self.action_areas.clear();
        let mut y = inner.y;
        for (index, (row, height)) in rows
            .iter()
            .zip(heights)
            .enumerate()
            .skip(self.list_state.offset())
        {
            if y >= inner.bottom() {
                break;
            }
            let visible = (*height).min(inner.bottom() - y);
            self.card_areas
                .push((Rect::new(inner.x, y, inner.width, visible), row.id));
            if Some(index) == focused {
                let action_y = y + height - 1;
                let mut x = inner.x + CARD_INDENT.len() as u16;
                for (action, label) in CARD_ACTIONS {
                    let width = label.len() as u16;
                    if action_y < inner.bottom() && x + width <= inner.right() {
                        self.action_areas
                            .push((Rect::new(x, action_y, width, 1), action));
                    }
                    x += width + 1;
                }
            }
            y += height;
        }
    }

    /// Wide terminals only: the focused item's details beside the list instead of inside it.
    fn render_details(&self, frame: &mut Frame, area: Rect) {
        let rows = self.rows();
        let mut lines = Vec::new();
        let (title, details) = match rows.iter().find(|row| Some(row.id) == self.focus) {
            Some(row) => {
                lines.push(Line::styled(row.title.clone(), theme::bold()));
                lines.push(Line::styled(row.kind, theme::colored(row.color)));
                lines.push(Line::default());
                let details = self.details_lines(row);
                let pages = details.len().div_ceil(DETAIL_PAGE_LINES).max(1);
                let page = self.detail_page.min(pages - 1);
                let title = if pages > 1 {
                    format!("Details · page {}/{} · [ ]", page + 1, pages)
                } else {
                    "Details".to_owned()
                };
                (
                    title,
                    details
                        .into_iter()
                        .skip(page * DETAIL_PAGE_LINES)
                        .take(DETAIL_PAGE_LINES)
                        .collect(),
                )
            }
            None => (
                "Details".to_owned(),
                vec![Line::styled(EMPTY_DETAILS, theme::hint())],
            ),
        };
        lines.extend(details);
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::bordered().title(title))
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0)),
            area,
        );
    }

    fn render_editor(&self, frame: &mut Frame, area: Rect) {
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
            .push(Span::styled("▏", theme::colored(theme::PRIMARY)));
        let mut tail = after.split('\n');
        current
            .spans
            .push(Span::raw(tail.next().unwrap_or("").to_owned()));
        lines.extend(tail.map(|line| Line::raw(line.to_owned())));
        let title = if self.input_mode == InputMode::Filter {
            "Filter".into()
        } else {
            format!(
                "Ask · {:?} · {} selected · generation {}",
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
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let message = if self.selection_stale() {
            Line::styled(
                "STALE selection: r clears; reselect before acting",
                theme::colored(theme::CHANGED),
            )
        } else if self.message.starts_with("Error") || self.message.starts_with("Refresh error") {
            Line::styled(
                list::single_line(&self.message),
                theme::colored(theme::ERROR),
            )
        } else {
            Line::raw(list::single_line(&self.message))
        };
        let hint = match self.input_mode {
            InputMode::Browse if self.graph_active() => {
                "Enter ask · click box or arrow · + - 0 zoom · wheel pan · s export · v list · ? help"
            }
            InputMode::Browse => {
                "Enter ask · drag text then Enter · h hide · x drop · v diagram · ? help · q quit"
            }
            InputMode::Question => {
                "Ctrl-S send · Ctrl-N delivery mode · Enter newline · Esc keep draft"
            }
            InputMode::Filter => "Type filter · Enter or Esc finish",
        };
        frame.render_widget(
            Paragraph::new(vec![message, Line::styled(hint, theme::hint())]),
            area,
        );
    }

    fn detail_page_count(&self) -> usize {
        self.rows()
            .iter()
            .find(|row| Some(row.id) == self.focus)
            .map_or(1, |row| {
                self.details_lines(row)
                    .len()
                    .div_ceil(DETAIL_PAGE_LINES)
                    .max(1)
            })
    }

    /// Everything about one item below its title: text, source, its questions and its id.
    fn details_lines(&self, row: &list::Row) -> Vec<Line<'static>> {
        let mut lines: Vec<Line> = row
            .details
            .split('\n')
            .map(|line| Line::raw(line.to_owned()))
            .collect();
        for question in self
            .questions
            .iter()
            .filter(|question| question.snapshot.selection.ids.contains(&row.id))
        {
            let text = terminal_safe(&question.text);
            let mut parts = text.split('\n');
            lines.push(Line::from(vec![
                Span::styled("? ", theme::colored(theme::QUESTION)),
                Span::raw(parts.next().unwrap_or("").to_owned()),
            ]));
            lines.extend(parts.map(|line| Line::raw(format!("  {line}"))));
            lines.push(Line::styled(
                format!("  {}", outcome_label(question)),
                theme::colored(outcome_color(question)),
            ));
        }
        lines.push(Line::styled(format!("id {}", row.id), theme::hint()));
        lines
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardAction {
    Ask,
    Hide,
    Drop,
}

const ASK_REFERENCE_PREFIX: &str = "Item";

/// Drop is the shared archive under the name users see; `u` undoes it.
const CARD_ACTIONS: [(CardAction, &str); 3] = [
    (CardAction::Ask, "[ask]"),
    (CardAction::Hide, "[hide]"),
    (CardAction::Drop, "[drop]"),
];

/// Truncates by character count with an ellipsis; the terminal clips any remaining overflow.
fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut clipped: String = text.chars().take(width - 1).collect();
    clipped.push('…');
    clipped
}

/// The focused card is marked by a colored bar in the left gutter, not by a background.
fn gutter(focused: bool) -> Span<'static> {
    if focused {
        Span::styled("▌ ", theme::colored(theme::PRIMARY))
    } else {
        Span::raw("  ")
    }
}

/// One card: header with gutter, number, kind, title and status; then either the compact
/// excerpt and latest question, or the expanded details window; and, for the focused card
/// of the active list, its clickable action pills.
#[allow(clippy::too_many_arguments)]
fn card_lines(
    row: &list::Row,
    number: usize,
    focused: bool,
    question: Option<&Question>,
    width: usize,
    actions: bool,
    expanded: Option<&[Line<'static>]>,
) -> Vec<Line<'static>> {
    let mut head = vec![
        gutter(focused),
        Span::styled(format!("{number} "), theme::bold()),
        Span::styled(format!("{} ", row.kind), theme::colored(row.color)),
    ];
    let used: usize = head.iter().map(Span::width).sum();
    let title_style = if focused {
        theme::bold()
    } else {
        Style::default()
    };
    head.push(Span::styled(
        clip(&row.title, width.saturating_sub(used)),
        title_style,
    ));
    let mut lines = vec![Line::from(head)];
    match expanded {
        Some(expanded) => lines.extend(expanded.iter().cloned()),
        None => {
            if !row.excerpt.is_empty() {
                lines.push(Line::from(vec![
                    gutter(focused),
                    Span::styled(
                        format!(
                            "  {}",
                            clip(&row.excerpt, width.saturating_sub(CARD_INDENT.len()))
                        ),
                        theme::hint(),
                    ),
                ]));
            }
            if let Some(question) = question {
                let text = list::single_line(
                    question
                        .text
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or(""),
                );
                let state = format!(" · {}", short_outcome(question));
                lines.push(Line::from(vec![
                    gutter(focused),
                    Span::styled("  ? ", theme::colored(theme::QUESTION)),
                    Span::raw(clip(
                        &text,
                        width.saturating_sub(CARD_INDENT.len() + 2 + state.chars().count()),
                    )),
                    Span::styled(state, theme::hint()),
                ]));
            }
        }
    }
    if actions && focused {
        let mut spans = vec![gutter(focused), Span::raw("  ")];
        for (action, label) in CARD_ACTIONS {
            let style = if action == CardAction::Ask {
                theme::bold().fg(theme::PRIMARY)
            } else {
                theme::bold()
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Greedy word wrap by character count that keeps each span's style; words longer than the
/// width are split. The prefix starts every produced line.
fn wrap_line(line: &Line<'static>, width: usize, prefix: &[Span<'static>]) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut current = prefix.to_vec();
    let mut used = 0;
    if width == 0 {
        current.extend(line.spans.iter().cloned());
        return vec![Line::from(current)];
    }
    for span in &line.spans {
        for word in words(&span.content) {
            if used + word.chars().count() > width && used > 0 {
                out.push(Line::from(std::mem::replace(&mut current, prefix.to_vec())));
                used = 0;
                if word.trim().is_empty() {
                    continue;
                }
            }
            let mut rest = word;
            while rest.chars().count() > width {
                let head: String = rest.chars().take(width).collect();
                rest = rest.chars().skip(width).collect();
                current.push(Span::styled(head, span.style));
                out.push(Line::from(std::mem::replace(&mut current, prefix.to_vec())));
            }
            used += rest.chars().count();
            current.push(Span::styled(rest, span.style));
        }
    }
    out.push(Line::from(current));
    out
}

/// Splits text into alternating runs of non-space and space characters, both kept.
fn words(text: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    for character in text.chars() {
        match runs.last_mut() {
            Some(run)
                if run.chars().next().is_some_and(char::is_whitespace)
                    == character.is_whitespace() =>
            {
                run.push(character)
            }
            _ => runs.push(character.to_string()),
        }
    }
    runs
}

fn help_lines(sections: &[(&'static str, &[&'static str])]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, entries) in sections {
        lines.push(Line::styled(*title, theme::bold().fg(theme::PRIMARY)));
        lines.extend(entries.iter().map(|entry| Line::raw(*entry)));
        lines.push(Line::default());
    }
    lines
}

fn render_help(frame: &mut Frame, area: Rect, scroll: u16) {
    let block = Block::bordered().title("Help · j/k scroll · Esc close");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < HELP_TWO_COLUMN_WIDTH {
        frame.render_widget(
            Paragraph::new(help_lines(HELP_SECTIONS))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            inner,
        );
        return;
    }
    let split = HELP_SECTIONS.len().div_ceil(2);
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);
    for (sections, column) in [
        (&HELP_SECTIONS[..split], columns[0]),
        (&HELP_SECTIONS[split..], columns[1]),
    ] {
        frame.render_widget(
            Paragraph::new(help_lines(sections))
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0)),
            column,
        );
    }
}

fn short_outcome(question: &Question) -> &'static str {
    match &question.outcome {
        DeliveryOutcome::Pending if question.mode == DeliveryMode::Share => "shared",
        DeliveryOutcome::Pending => "pending",
        DeliveryOutcome::Submitted => "sent",
        DeliveryOutcome::Queued { .. } => "queued",
        DeliveryOutcome::Started { .. } => "started",
        DeliveryOutcome::Failed { .. } => "failed",
        DeliveryOutcome::Refused { .. } => "refused",
        DeliveryOutcome::Dropped { .. } => "dropped",
        DeliveryOutcome::Unknown { .. } => "unknown",
    }
}

fn outcome_color(question: &Question) -> Color {
    match &question.outcome {
        DeliveryOutcome::Failed { .. }
        | DeliveryOutcome::Refused { .. }
        | DeliveryOutcome::Dropped { .. } => theme::ERROR,
        DeliveryOutcome::Queued { .. } | DeliveryOutcome::Unknown { .. } => theme::CHANGED,
        DeliveryOutcome::Started { .. } => theme::ACCEPTED,
        DeliveryOutcome::Pending | DeliveryOutcome::Submitted => theme::HINT,
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

const HELP_SECTIONS: &[(&str, &[&str])] = &[
    (
        "Navigate",
        &[
            "j/k or arrows: move focus",
            "Home / End: first / last item",
            "PgUp / PgDn: scroll details",
            "[ / ]: previous / next details page",
        ],
    ),
    (
        "Select and act",
        &[
            "Space or click: toggle selection",
            "A: select all visible items",
            "r: clear selection",
            "a: accept (focused item when nothing is selected)",
            "x: archive",
            "u: undo last action",
            "h: hide selected in this view",
            "H: show hidden items",
            "/: edit private filter",
        ],
    ),
    (
        "Ask",
        &[
            "e or [ask]: write a question about the selection",
            "Enter: newline",
            "Ctrl-S: send (Share)",
            "Ctrl-N: delivery mode",
            "Esc: keep draft / finish filter",
            "Left / Right / Home / End, Backspace / Delete: edit text",
        ],
    ),
    (
        "Views",
        &[
            "Tab / Shift-Tab: switch view",
            "Ctrl-T: new view of this document",
            "Ctrl-W: close view (content and draft kept)",
            "Ctrl-R: reopen closed view",
            "Ctrl-\\ (Ctrl-4 in most terminals): toggle split",
            "F6 or click: focus other split",
            "v: list / diagram",
        ],
    ),
    (
        "Diagram",
        &[
            "Click box, arrow, pill or badge: focus it",
            "+ / - / 0: zoom in, out, fit",
            "Wheel: pan · Shift-wheel: sideways · Ctrl-wheel: zoom",
            "Shift-arrows: pan",
            "s: export SVG to .prism/export",
            "Ctrl-L: repaint (after tmux redraws over the image)",
            "Alt-arrows: move card (text diagram only)",
        ],
    ),
    (
        "Notes",
        &[
            "q / Ctrl-C: quit",
            "Acceptance is feedback, never tool permission.",
            "Share creates a snapshot for explicit retrieval in the bound CLI; it does not start a turn.",
            "Unknown delivery is never retried automatically; sending again can duplicate it.",
        ],
    ),
];

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

/// Caches the bridge probe: each probe spawns tmux checks and an HTTP health call, so idle
/// redraws and ordinary keystrokes reuse the last result until the refresh interval passes.
pub struct ConnectionProbe<F: FnMut() -> ConnectionInfo> {
    status: F,
    cached: Option<(Instant, ConnectionInfo)>,
}

impl<F: FnMut() -> ConnectionInfo> ConnectionProbe<F> {
    pub fn new(status: F) -> Self {
        Self {
            status,
            cached: None,
        }
    }

    pub fn current(&mut self, force: bool) -> ConnectionInfo {
        if let Some((probed, connection)) = &self.cached
            && !force
            && probed.elapsed() < REFRESH_INTERVAL
        {
            return connection.clone();
        }
        let connection = (self.status)();
        self.cached = Some((Instant::now(), connection.clone()));
        connection
    }
}

/// How long the event loop gets to leave on its own after a termination signal.
const EXIT_GRACE: Duration = Duration::from_millis(500);
const EXIT_WATCH_INTERVAL: Duration = Duration::from_millis(100);

/// When the pane closes, reading the dead terminal can spin inside the input library and
/// never return to the loop that checks the signal flag. The watchdog ends the process so a
/// closed pane never leaves a UI running.
fn spawn_exit_watchdog(interrupted: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !interrupted.load(Ordering::Relaxed) {
            std::thread::sleep(EXIT_WATCH_INTERVAL);
        }
        std::thread::sleep(EXIT_GRACE);
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
        std::process::exit(0);
    });
}

/// Terminals known to draw iTerm2 inline images, matched against the outer terminal name.
const IMAGE_TERMINALS: [&str; 2] = ["iTerm", "WezTerm"];
/// Raster resolution per cell when the terminal reports no pixel size (common inside tmux).
const DEFAULT_CELL_PIXELS: (u16, u16) = (10, 20);

/// The terminal Prism is ultimately drawn by: tmux knows its client's type; otherwise the
/// terminal announces itself through the environment. Nothing is queried over stdio.
fn outer_terminal(tmux: bool) -> String {
    if tmux {
        return std::process::Command::new("tmux")
            .args(["display-message", "-p", "#{client_termtype}"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
    }
    ["TERM_PROGRAM", "LC_TERMINAL"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `PRISM_CELL_PIXELS=WxH` wins, then the terminal's reported pixel size, then a default.
fn cell_pixels() -> (u16, u16) {
    let configured = std::env::var("PRISM_CELL_PIXELS").ok().and_then(|value| {
        let (width, height) = value.split_once('x')?;
        Some((width.parse().ok()?, height.parse().ok()?))
    });
    let reported = crossterm::terminal::window_size().ok().and_then(|size| {
        (size.width > 0 && size.height > 0 && size.columns > 0 && size.rows > 0)
            .then(|| (size.width / size.columns, size.height / size.rows))
    });
    configured
        .or(reported)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or(DEFAULT_CELL_PIXELS)
}

/// Graphic diagrams need a terminal that draws inline images; `PRISM_TEXT_DIAGRAM` forces
/// the text diagram.
fn graph_view() -> Option<graph::GraphView> {
    if std::env::var_os("PRISM_TEXT_DIAGRAM").is_some() {
        return None;
    }
    let tmux = std::env::var_os("TMUX").is_some_and(|value| !value.is_empty());
    let terminal = outer_terminal(tmux);
    if !IMAGE_TERMINALS.iter().any(|name| terminal.contains(name)) {
        return None;
    }
    Some(graph::GraphView::new(
        tmux,
        cell_pixels(),
        graph::load_fonts()?,
    ))
}

/// Control-key presses carry the explicit actions (Share) that must see a current binding.
pub fn requires_verified_binding(event: &Event) -> bool {
    matches!(event, Event::Key(key) if key.modifiers.contains(KeyModifiers::CONTROL))
}

pub fn run(
    mut store: Store,
    workspace: Workspace,
    status: impl FnMut() -> ConnectionInfo,
) -> Result<()> {
    let mut state = UiState::new(&mut store, workspace)?;
    state.set_graph(graph_view());
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
    spawn_exit_watchdog(Arc::clone(&interrupted));
    let mut probe = ConnectionProbe::new(status);
    let mut refreshed = Instant::now();
    while !interrupted.load(Ordering::Relaxed) {
        let connection = probe.current(false);
        if refreshed.elapsed() >= REFRESH_INTERVAL {
            if let Err(error) = state.refresh(&mut store) {
                state.message = format!("Refresh error: {error}");
            }
            refreshed = Instant::now();
        }
        terminal.draw(|frame| state.render(frame, &connection))?;
        if state.take_redraw() {
            terminal.clear()?;
            terminal.draw(|frame| state.render(frame, &connection))?;
        }
        if event::poll(EVENT_WAIT)? {
            let event = event::read()?;
            // Refresh the binding immediately before explicit actions, not just before drawing.
            let connection = probe.current(requires_verified_binding(&event));
            if state.handle_event(&mut store, event, &connection) {
                break;
            }
        }
    }
    drop(guard);
    Ok(())
}
