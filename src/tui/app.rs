//! The dashboard's state machine: `update` is pure and returns the effects
//! (jump to a pane, notify, quit) for the runner to carry out.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::model::State;
use crate::store::queries::{Overview, OverviewEntry, OverviewQuery, Section};
use crate::store::Store;

const NO_PANE_STATUS: &str = "no tmux pane recorded for this session";

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub overview: Overview,
    /// Session id to the tmux pane it last reported.
    pub panes: HashMap<String, String>,
    pub now: String,
}

impl Snapshot {
    /// Reads the default overview and the recorded panes; `now` dates the
    /// finished-session collapse and the age column.
    pub fn load(store: &Store, now: &str) -> Result<Self, String> {
        let overview = store
            .overview(&OverviewQuery::default(), now)
            .map_err(|e| e.to_string())?;
        let panes = store
            .list_sessions()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter_map(|session| session.tmux_pane.map(|pane| (session.id, pane)))
            .collect();
        Ok(Self {
            overview,
            panes,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    JumpToPane { pane: String },
    Notify { project: String, state: State },
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
}

impl App {
    pub fn new() -> Self {
        Self {
            snapshot: None,
            selected: None,
            hidden: HashSet::new(),
            seen: HashMap::new(),
            status: None,
        }
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

    /// Each section with the entries that are not hidden, in display order.
    pub fn sections(&self) -> Vec<(&Section, Vec<&OverviewEntry>)> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .overview
            .sections
            .iter()
            .map(|section| {
                let shown = section
                    .sessions
                    .iter()
                    .filter(|entry| !self.is_hidden(entry))
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

    fn on_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        // Ctrl-D reaching the pane (an EOF on attach) must not act as `d`.
        if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return Vec::new();
        }
        self.status = None;
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return vec![Effect::Quit],
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(true),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(false),
            KeyCode::Enter => return self.jump(),
            KeyCode::Char('d') => self.hide_selected(),
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
        effects
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
    };
    (app, effects)
}
