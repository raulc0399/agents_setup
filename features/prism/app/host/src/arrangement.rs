//! Windows, their tabs (one per conversation), which window each project opens in, the
//! closed tabs that can be reopened, each tab's view state, each window's side pane width and
//! which conversations ended.
//! Persisted as JSON in the app's data directory and restored at start; content never is.
use crate::files::write_atomic;
use crate::registry::SessionKey;
use prism_core::model::AgentKind;
use prism_core::protocol::Session;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

pub const ARRANGEMENT_FILE: &str = "arrangement.json";
/// Where an unreadable arrangement is kept for the user before starting empty.
pub const CORRUPT_ARRANGEMENT_FILE: &str = "arrangement.corrupt.json";

/// Closed tabs remembered for reopening, most recent first.
pub const MAX_CLOSED_TABS: usize = 50;

pub type WindowId = String;

/// Window labels are `w1`, `w2`, ...; Tauri accepts them as window labels.
const WINDOW_PREFIX: &str = "w";

/// What a tab shows before (or without) its bridge connecting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLabel {
    pub host: String,
    pub project: String,
    pub agent: AgentKind,
    pub conversation: String,
}

impl From<&Session> for SessionLabel {
    fn from(session: &Session) -> Self {
        Self {
            host: session.host.clone(),
            project: session.project.clone(),
            agent: session.agent.clone(),
            conversation: session.conversation.clone(),
        }
    }
}

/// How a conversation ended (its bridge sent `closed`), so that after a restart its tab says
/// so instead of waiting for a bridge that never comes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ending {
    pub reason: String,
    /// Milliseconds since the Unix epoch.
    pub ended_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub id: WindowId,
    pub tabs: Vec<SessionKey>,
    pub active: Option<SessionKey>,
    /// The width of the side pane in CSS pixels, owned by the UI (it applies the limits);
    /// none shows its default. Missing in files saved before it was kept.
    #[serde(default)]
    pub pane_width: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CloseWindow {
    Closed,
    /// The only window stays as it is; closing it quits the app.
    Last,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Arrangement {
    /// In creation order.
    pub windows: Vec<Window>,
    /// The window a project's new conversations open in.
    pub project_windows: BTreeMap<String, WindowId>,
    /// Tabs the user closed, most recent first.
    pub closed: Vec<SessionKey>,
    pub labels: BTreeMap<SessionKey, SessionLabel>,
    /// Per tab, owned by the UI (document, focus, hidden items, view mode).
    pub views: BTreeMap<SessionKey, serde_json::Value>,
    /// Tabs whose conversation ended; a bridge connecting again clears the mark.
    pub ended: BTreeMap<SessionKey, Ending>,
    /// When each tab's bridge was last connected, in milliseconds since the Unix epoch:
    /// written when it disconnects and when the app quits, cleared when one connects.
    pub seen: BTreeMap<SessionKey, u64>,
    /// The window the user focused last; new projects open there.
    pub focused: Option<WindowId>,
    next_window: u64,
}

impl Arrangement {
    /// Reads the saved arrangement. A missing file starts empty; an unreadable one too, after
    /// a copy of it was kept next to it, and the message says so.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let error = match fs::read(path) {
            Ok(contents) => match serde_json::from_slice(&contents) {
                Ok(arrangement) => return (arrangement, None),
                Err(error) => error.to_string(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => error.to_string(),
        };
        let copy = path.with_file_name(CORRUPT_ARRANGEMENT_FILE);
        let kept = match fs::copy(path, &copy) {
            Ok(_) => format!("a copy is in {}", copy.display()),
            Err(copy_error) => format!("no copy could be kept: {copy_error}"),
        };
        let message = format!(
            "The window arrangement in {} could not be read ({error}); {kept}. Prism started \
             with one empty window.",
            path.display()
        );
        (Self::default(), Some(message))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        write_atomic(path, &contents, false)
    }

    pub fn path(data_directory: &Path) -> PathBuf {
        data_directory.join(ARRANGEMENT_FILE)
    }

    /// At start: at least one window.
    pub fn ensure_window(&mut self) {
        if self.windows.is_empty() {
            self.new_window();
        }
    }

    pub fn window(&self, id: &str) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn window_of(&self, key: &str) -> Option<&Window> {
        self.windows
            .iter()
            .find(|window| window.tabs.iter().any(|tab| tab == key))
    }

    /// Opens a tab for a conversation seen for the first time: in its project's window if
    /// that still exists, otherwise in the window focused last. A tab the user closed stays
    /// closed. Returns the window a new tab opened in; none without any window.
    pub fn place(&mut self, key: &str, label: SessionLabel) -> Option<WindowId> {
        let project = label.project.clone();
        self.labels.insert(key.to_owned(), label);
        if self.window_of(key).is_some() || self.closed.iter().any(|closed| closed == key) {
            return None;
        }

        let remembered = self
            .project_windows
            .get(&project)
            .filter(|id| self.window(id).is_some())
            .cloned();
        let focused = self.focused.clone().filter(|id| self.window(id).is_some());
        let first = self.windows.first().map(|window| window.id.clone());
        let target = remembered.or(focused).or(first)?;
        self.add_tab(&target, key, false);
        self.project_windows
            .entry(project)
            .or_insert(target.clone());
        Some(target)
    }

    pub fn activate(&mut self, window: &str, key: &str) {
        if let Some(window) = self.window_mut(window)
            && window.tabs.iter().any(|tab| tab == key)
        {
            window.active = Some(key.to_owned());
        }
    }

    pub fn close_tab(&mut self, key: &str) {
        if !self.remove_tab(key) {
            return;
        }
        self.closed.retain(|closed| closed != key);
        self.closed.insert(0, key.to_owned());
        self.forget_beyond_closed_limit();
    }

    /// Opens a closed tab (or moves an open one) into `window` and shows it.
    pub fn reopen(&mut self, key: &str, label: SessionLabel, window: &str) {
        if self.window(window).is_none() {
            return;
        }
        self.labels.insert(key.to_owned(), label);
        self.closed.retain(|closed| closed != key);
        self.remove_tab(key);
        self.add_tab(window, key, true);
    }

    /// Moves a tab to `target`, or to a new window when `target` is `None` or gone, and
    /// remembers that window for the tab's project. Returns the target window.
    pub fn move_tab(&mut self, key: &str, target: Option<&str>) -> Option<WindowId> {
        self.window_of(key)?;
        let target = match target.filter(|id| self.window(id).is_some()) {
            Some(id) => id.to_owned(),
            None => self.new_window(),
        };
        self.remove_tab(key);
        self.add_tab(&target, key, true);
        if let Some(label) = self.labels.get(key) {
            self.project_windows
                .insert(label.project.clone(), target.clone());
        }
        Some(target)
    }

    /// The user closed a window: its tabs become closed tabs. The last window stays.
    pub fn close_window(&mut self, id: &str) -> CloseWindow {
        if self.windows.len() <= 1 {
            return CloseWindow::Last;
        }
        let Some(position) = self.windows.iter().position(|window| window.id == id) else {
            return CloseWindow::Closed;
        };
        let window = self.windows.remove(position);
        for key in window.tabs.iter().rev() {
            self.closed.retain(|closed| closed != key);
            self.closed.insert(0, key.clone());
        }
        self.forget_beyond_closed_limit();
        self.project_windows.retain(|_, window| window != id);
        if self.focused.as_deref() == Some(id) {
            self.focused = None;
        }
        CloseWindow::Closed
    }

    pub fn focus(&mut self, id: &str) {
        if self.window(id).is_some() {
            self.focused = Some(id.to_owned());
        }
    }

    pub fn set_pane_width(&mut self, id: &str, width: Option<u32>) {
        if let Some(window) = self.window_mut(id) {
            window.pane_width = width;
        }
    }

    pub fn set_view(&mut self, key: &str, view: serde_json::Value) {
        if self.labels.contains_key(key) {
            self.views.insert(key.to_owned(), view);
        }
    }

    /// Remembers that a known tab's conversation ended.
    pub fn end(&mut self, key: &str, ending: Ending) {
        if self.labels.contains_key(key) {
            self.ended.insert(key.to_owned(), ending);
        }
    }

    /// Remembers when a known tab's bridge was last connected.
    pub fn see(&mut self, key: &str, at_ms: u64) {
        if self.labels.contains_key(key) {
            self.seen.insert(key.to_owned(), at_ms);
        }
    }

    /// A bridge connected for the conversation (for example on resume): it no longer ended
    /// and is connected now.
    pub fn resume(&mut self, key: &str) {
        self.ended.remove(key);
        self.seen.remove(key);
    }

    /// `Prism · <project names of its tabs>`, for the native window title.
    pub fn window_title(&self, id: &str) -> String {
        let mut projects: Vec<&str> = Vec::new();
        for key in self
            .window(id)
            .map(|window| window.tabs.as_slice())
            .unwrap_or_default()
        {
            if let Some(label) = self.labels.get(key) {
                let name = project_name(&label.project);
                if !projects.contains(&name) {
                    projects.push(name);
                }
            }
        }
        if projects.is_empty() {
            return "Prism".into();
        }
        format!("Prism · {}", projects.join(", "))
    }

    fn new_window(&mut self) -> WindowId {
        // The counter is persisted, so a label is never reused and a stale reference (a
        // native window still closing) never points at a different window.
        self.next_window += 1;
        let id = format!("{WINDOW_PREFIX}{}", self.next_window);
        self.windows.push(Window {
            id: id.clone(),
            tabs: Vec::new(),
            active: None,
            pane_width: None,
        });
        id
    }

    fn window_mut(&mut self, id: &str) -> Option<&mut Window> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    /// A window without an active tab shows the new one; `show` always does.
    fn add_tab(&mut self, window: &str, key: &str, show: bool) {
        let Some(window) = self.window_mut(window) else {
            return;
        };
        window.tabs.push(key.to_owned());
        if show || window.active.is_none() {
            window.active = Some(key.to_owned());
        }
    }

    /// Removes the tab from its window; the neighbour becomes active if it was.
    fn remove_tab(&mut self, key: &str) -> bool {
        let Some(window) = self
            .windows
            .iter_mut()
            .find(|window| window.tabs.iter().any(|tab| tab == key))
        else {
            return false;
        };
        let position = window
            .tabs
            .iter()
            .position(|tab| tab == key)
            .expect("the window holds the tab");
        window.tabs.remove(position);
        if window.active.as_deref() == Some(key) {
            window.active = window
                .tabs
                .get(position)
                .or_else(|| window.tabs.last())
                .cloned();
        }
        true
    }

    /// Labels, views, ended marks and last-seen times of tabs neither open nor remembered as
    /// closed are dropped.
    fn forget_beyond_closed_limit(&mut self) {
        self.closed.truncate(MAX_CLOSED_TABS);
        let kept = |key: &SessionKey, windows: &[Window], closed: &[SessionKey]| {
            closed.contains(key) || windows.iter().any(|window| window.tabs.contains(key))
        };
        let (windows, closed) = (&self.windows, &self.closed);
        self.labels.retain(|key, _| kept(key, windows, closed));
        self.views.retain(|key, _| kept(key, windows, closed));
        self.ended.retain(|key, _| kept(key, windows, closed));
        self.seen.retain(|key, _| kept(key, windows, closed));
    }
}

/// Last path component of a project directory, as tabs and window titles show it.
pub fn project_name(project: &str) -> &str {
    project
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(project)
}
