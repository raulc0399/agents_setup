pub mod diagram;
pub mod list;

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
}

impl UiState {
    pub fn new(store: &mut Store, workspace: Workspace) -> Result<Self> {
        let mut state = Self {
            workspace,
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
        if self.view.is_none()
            && let Some(document) = self.documents.first()
        {
            self.load_document(store, document.id)?;
        }
        self.reconcile_focus();
        Ok(())
    }

    fn load_document(&mut self, store: &mut Store, document_id: Id) -> Result<()> {
        let saved = store
            .views(self.workspace.id)?
            .into_iter()
            .find(|view| view.document_id == document_id && view.kind == ViewKind::List);
        let view = saved.unwrap_or_else(|| ViewState {
            id: Id::new_v4(),
            document_id,
            kind: ViewKind::List,
            selection: None,
            positions: BTreeMap::new(),
            hidden: BTreeSet::new(),
            filter: String::new(),
            draft: String::new(),
        });
        store.save_view(self.workspace.id, &view)?;
        self.focus = view
            .selection
            .as_ref()
            .and_then(|selection| selection.ids.first().copied());
        self.editor_cursor = view.draft.len();
        self.view = Some(view);
        self.list_state = ListState::default();
        self.detail_scroll = 0;
        self.detail_page = 0;
        self.reconcile_focus();
        Ok(())
    }

    fn reconcile_focus(&mut self) {
        let rows = self.rows();
        let index = rows
            .iter()
            .position(|row| Some(row.id) == self.focus)
            .or_else(|| (!rows.is_empty()).then_some(0));
        self.focus = index.map(|index| rows[index].id);
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
            Ok(exit) => exit,
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
            if self.input_mode == InputMode::Browse
                && self.list_area.contains((mouse.column, mouse.row).into())
            {
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
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('?') => {
                self.show_help = true;
                self.detail_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_focus(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_focus(-1),
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
                    ids: self.rows().iter().map(|row| row.id).collect(),
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
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(document) = self.document() {
                    let index = self
                        .documents
                        .iter()
                        .position(|item| item.id == document.id)
                        .unwrap_or(0);
                    let next = if key.code == KeyCode::BackTab {
                        (index + self.documents.len() - 1) % self.documents.len()
                    } else {
                        (index + 1) % self.documents.len()
                    };
                    self.load_document(store, self.documents[next].id)?;
                }
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
        let mut cursor = self.editor_cursor.min(text.len());
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
            Constraint::Length(2),
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
        let header =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[0]);
        frame.render_widget(Paragraph::new(format!("Prism: {title}")), header[0]);
        frame.render_widget(Paragraph::new(status), header[1]);
        if self.show_help {
            self.list_area = Rect::default();
            let controls = "j/k or arrows: move focus\nSpace or click: toggle selection\nA: select all visible content\nr: clear stale/current selection\na: accept shared content\nx: archive shared content\nu: undo last content action\nh: hide selected in this view\nH: show all hidden content\n/: edit private filter\nTab / Shift-Tab: switch document\nPgUp/PgDn: scroll details/delivery\n[ / ]: previous/next details page\ne: edit private question draft\nEnter in editor: newline\nCtrl-S or Ctrl-Enter: explicit send\nCtrl-N in editor: delivery mode\nEsc: keep private draft / finish filter\nLeft/Right/Home/End: edit cursor\nBackspace/Delete: edit text\nq / Ctrl-C: quit\n\nAcceptance is feedback, never tool permission.\nShare creates a snapshot for explicit retrieval in the bound CLI; it does not start a turn.\nUnknown delivery is never automatically retried. Sending it again can duplicate the request.";
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
        self.render_list(frame, panes[0]);
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
            let cursor = self.editor_cursor.min(text.len());
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
