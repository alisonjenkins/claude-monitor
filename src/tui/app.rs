//! The dashboard's state machine: `update` is pure and returns the effects
//! (jump to a pane, notify, quit) for the runner to carry out.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};

use super::detail::DetailView;
use super::restore::{PreviewRow, RestoreResult, RestoreView, Step};
use crate::cli::detail::SessionDetail;
use crate::export::links::Check;
use crate::model::State;
use crate::store::queries::{Overview, OverviewEntry, OverviewQuery, Section, SectionName};
use crate::store::Store;

const NO_PANE_STATUS: &str = "no tmux pane recorded for this session";
const NO_LINKS_STATUS: &str = "this session has no links to check";

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub overview: Overview,
    /// Session id to the tmux pane it last reported.
    pub panes: HashMap<String, String>,
    /// Session id to the stored `(done, total)` of its bound plan.
    pub figures: HashMap<String, (u32, u32)>,
    pub now: String,
}

impl Snapshot {
    /// Reads the default overview and the recorded panes; `now` dates the age
    /// column.
    pub fn load(store: &Store, now: &str) -> Result<Self, String> {
        let overview = store
            .overview(&OverviewQuery::default())
            .map_err(|e| e.to_string())?;
        let panes = store
            .list_sessions()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|session| session.tmux_pane.map(|pane| (session.id, pane)))
            .collect();
        let figures = store.progress_figures().map_err(|e| e.to_string())?;
        Ok(Self {
            overview,
            panes,
            figures,
            now: now.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Key(KeyCode, KeyModifiers),
    Refreshed(Snapshot),
    /// An effect or a read failed; shown in the footer.
    Failed(String),
    /// The plans for the restore view's confirm screen.
    RestorePreview(Vec<PreviewRow>),
    /// One session of a confirmed restore started or was refused.
    RestoreResult(RestoreResult),
    /// The answer to [`Effect::LoadDetail`].
    Detail(SessionDetail),
    /// The answer to [`Effect::CheckLinks`]: the session's links compared with
    /// their remote nodes, or why the check could not run.
    LinksChecked {
        id: String,
        result: Result<Check, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Plan these sessions and answer with [`Msg::RestorePreview`].
    PreviewRestore {
        ids: Vec<String>,
    },
    /// Start these sessions and answer with a [`Msg::RestoreResult`] each.
    StartRestore {
        ids: Vec<String>,
    },
    /// Read this session's detail and answer with [`Msg::Detail`].
    LoadDetail {
        id: String,
    },
    /// Compare this session's links with their remote nodes off the UI thread
    /// and answer with [`Msg::LinksChecked`].
    CheckLinks {
        id: String,
    },
    JumpToPane {
        pane: String,
    },
    Notify {
        project: String,
        state: State,
    },
    Quit,
}

#[derive(Debug, Clone)]
pub struct App {
    snapshot: Option<Snapshot>,
    selected: Option<String>,
    /// Hidden with `d`: the session id and the state it was in. UI-local, so a
    /// new dashboard shows everything again.
    hidden: HashSet<(String, State)>,
    seen: HashMap<String, State>,
    status: Option<String>,
    /// Toggled with `u`: the unseen section lists its rows only when set.
    unseen_expanded: bool,
    /// Open with `R`; takes over the keys until closed.
    restore: Option<RestoreView>,
    /// Open with `i`; takes over the keys until closed.
    detail: Option<DetailView>,
}

impl App {
    pub fn new() -> Self {
        Self {
            snapshot: None,
            selected: None,
            hidden: HashSet::new(),
            seen: HashMap::new(),
            status: None,
            unseen_expanded: false,
            restore: None,
            detail: None,
        }
    }

    pub fn detail(&self) -> Option<&DetailView> {
        self.detail.as_ref()
    }

    pub fn restore(&self) -> Option<&RestoreView> {
        self.restore.as_ref()
    }

    pub fn unseen_expanded(&self) -> bool {
        self.unseen_expanded
    }

    /// `None` until the first load.
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Each section with the entries that are not hidden, in display order. A
    /// collapsed unseen section shows none.
    pub fn sections(&self) -> Vec<(&Section, Vec<&OverviewEntry>)> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .overview
            .sections
            .iter()
            .map(|section| {
                let collapsed = section.name == SectionName::Unseen && !self.unseen_expanded;
                let shown = section
                    .sessions
                    .iter()
                    .filter(|entry| !collapsed && !self.is_hidden(entry))
                    .collect();
                (section, shown)
            })
            .collect()
    }

    /// Entries in display order, hidden ones left out.
    pub fn visible(&self) -> Vec<&OverviewEntry> {
        self.sections()
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .collect()
    }

    pub fn hidden_count(&self) -> usize {
        self.hidden.len()
    }

    fn is_hidden(&self, entry: &OverviewEntry) -> bool {
        self.hidden.contains(&(entry.id.clone(), entry.state))
    }

    fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        self.visible().iter().position(|entry| entry.id == selected)
    }

    fn select_index(&mut self, index: usize) {
        let id = {
            let visible = self.visible();
            visible
                .get(index.min(visible.len().saturating_sub(1)))
                .map(|entry| entry.id.clone())
        };
        self.selected = id;
    }

    fn keep_selection(&mut self, fallback_index: usize) {
        if self.selected_index().is_none() {
            self.select_index(fallback_index);
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let current = self.selected_index().unwrap_or(0);
        let next = if forward {
            current.saturating_add(1)
        } else {
            current.saturating_sub(1)
        };
        self.select_index(next);
    }

    fn jump(&mut self) -> Vec<Effect> {
        let Some(selected) = self.selected.clone() else {
            return Vec::new();
        };
        let pane = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.panes.get(&selected));
        match pane {
            Some(pane) => vec![Effect::JumpToPane { pane: pane.clone() }],
            None => {
                self.status = Some(NO_PANE_STATUS.to_owned());
                Vec::new()
            }
        }
    }

    fn hide_selected(&mut self) {
        let current = self.selected_index();
        let hidden = self.selected.as_deref().and_then(|selected| {
            self.visible()
                .into_iter()
                .find(|entry| entry.id == selected)
                .map(|entry| (entry.id.clone(), entry.state))
        });
        if let Some(key) = hidden {
            self.hidden.insert(key);
            self.keep_selection(current.unwrap_or(0));
        }
    }

    fn toggle_unseen(&mut self) {
        let current = self.selected_index();
        self.unseen_expanded = !self.unseen_expanded;
        self.keep_selection(current.unwrap_or(0));
    }

    fn open_restore(&mut self) {
        if let Some(snapshot) = &self.snapshot {
            self.restore = Some(RestoreView::from_overview(&snapshot.overview));
        }
    }

    fn on_restore_key(&mut self, code: KeyCode) -> Vec<Effect> {
        let Some(view) = self.restore.as_mut() else {
            return Vec::new();
        };
        match view.on_key(code) {
            Step::Stay => Vec::new(),
            Step::Close => {
                self.restore = None;
                Vec::new()
            }
            Step::Preview(ids) => vec![Effect::PreviewRestore { ids }],
            Step::Start(ids) => vec![Effect::StartRestore { ids }],
        }
    }

    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        // Ctrl-D reaching the pane (an EOF on attach) must not act as `d`.
        if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return Vec::new();
        }
        self.status = None;
        if self.restore.is_some() {
            return self.on_restore_key(code);
        }
        if self.detail.is_some() {
            return self.on_detail_key(code);
        }
        match code {
            KeyCode::Char('i') => return self.open_detail(),
            KeyCode::Char('L') => return self.open_detail_and_check_links(),
            KeyCode::Char('R') => self.open_restore(),
            KeyCode::Char('q') | KeyCode::Esc => return vec![Effect::Quit],
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(true),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(false),
            KeyCode::Enter => return self.jump(),
            KeyCode::Char('d') => self.hide_selected(),
            KeyCode::Char('u') => self.toggle_unseen(),
            _ => {}
        }
        Vec::new()
    }

    fn on_refreshed(&mut self, snapshot: Snapshot) -> Vec<Effect> {
        let entries: Vec<&OverviewEntry> = snapshot
            .overview
            .sections
            .iter()
            .flat_map(|section| section.sessions.iter())
            .collect();

        let current: HashSet<(String, State)> = entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.state))
            .collect();
        self.hidden.retain(|key| current.contains(key));

        // The first load only seeds `seen`: nothing is a transition yet.
        let effects = if self.snapshot.is_some() {
            entries
                .iter()
                .filter(|entry| is_notifiable(entry.state))
                .filter(|entry| self.seen.get(&entry.id) != Some(&entry.state))
                .map(|entry| Effect::Notify {
                    project: project_name(entry),
                    state: entry.state,
                })
                .collect()
        } else {
            Vec::new()
        };
        self.seen = entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.state))
            .collect();

        let previous = self.selected_index();
        self.snapshot = Some(snapshot);
        self.keep_selection(previous.unwrap_or(0));
        let mut effects = effects;
        if let Some(open) = &self.detail {
            effects.push(Effect::LoadDetail {
                id: open.id().to_owned(),
            });
        }
        effects
    }

    fn open_detail(&mut self) -> Vec<Effect> {
        let Some(id) = self.selected.clone() else {
            return Vec::new();
        };
        self.detail = Some(DetailView::loading(&id));
        vec![Effect::LoadDetail { id }]
    }

    fn open_detail_and_check_links(&mut self) -> Vec<Effect> {
        let mut effects = self.open_detail();
        if let Some(open) = self.detail.as_mut() {
            open.start_check();
            effects.push(Effect::CheckLinks {
                id: open.id().to_owned(),
            });
        }
        effects
    }

    fn on_detail_key(&mut self, code: KeyCode) -> Vec<Effect> {
        match code {
            KeyCode::Esc | KeyCode::Char('q' | 'i') => self.detail = None,
            KeyCode::Char('L') => return self.check_links(),
            _ => {}
        }
        Vec::new()
    }

    fn check_links(&mut self) -> Vec<Effect> {
        let Some(open) = self.detail.as_mut() else {
            return Vec::new();
        };
        if open.detail().is_some_and(|d| d.links.is_empty()) {
            self.status = Some(NO_LINKS_STATUS.to_owned());
            return Vec::new();
        }
        if !open.start_check() {
            return Vec::new();
        }
        vec![Effect::CheckLinks {
            id: open.id().to_owned(),
        }]
    }

    fn on_detail(&mut self, detail: SessionDetail) {
        if let Some(open) = self.detail.as_mut().filter(|open| open.id() == detail.id) {
            open.set(detail);
        }
    }

    fn on_links_checked(&mut self, id: &str, result: Result<Check, String>) {
        if let Some(open) = self.detail.as_mut().filter(|open| open.id() == id) {
            open.finish_check(result);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn is_notifiable(state: State) -> bool {
    matches!(state, State::NeedsPermission | State::Idle)
}

/// The repository name, else the last part of the working directory.
pub fn project_name(entry: &OverviewEntry) -> String {
    entry
        .repo
        .clone()
        .or_else(|| {
            Path::new(&entry.cwd)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| entry.id.clone())
}

pub fn update(mut app: App, msg: Msg) -> (App, Vec<Effect>) {
    let effects = match msg {
        Msg::Key(code, modifiers) => app.on_key(code, modifiers),
        Msg::Refreshed(snapshot) => app.on_refreshed(snapshot),
        Msg::Failed(message) => {
            app.status = Some(message);
            Vec::new()
        }
        Msg::RestorePreview(rows) => {
            if let Some(view) = app.restore.as_mut() {
                view.on_preview(rows);
            }
            Vec::new()
        }
        Msg::RestoreResult(result) => {
            if let Some(view) = app.restore.as_mut() {
                view.on_result(result);
            }
            Vec::new()
        }
        Msg::Detail(detail) => {
            app.on_detail(detail);
            Vec::new()
        }
        Msg::LinksChecked { id, result } => {
            app.on_links_checked(&id, result);
            Vec::new()
        }
    };
    (app, effects)
}
