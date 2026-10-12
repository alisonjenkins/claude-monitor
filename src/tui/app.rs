//! The dashboard's state machine: `update` is pure, effects are returned.

use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyModifiers};

use crate::model::State;
use crate::store::queries::{Overview, OverviewEntry, OverviewQuery};
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub overview: Overview,
    /// Session id to the tmux pane it last reported.
    pub panes: HashMap<String, String>,
    pub now: String,
}

impl Snapshot {
    pub fn load(store: &Store, now: &str) -> Result<Self, String> {
        let _ = (store, now, OverviewQuery::default());
        Ok(Self {
            overview: Overview {
                sections: Vec::new(),
            },
            panes: HashMap::new(),
            now: now.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Key(KeyCode, KeyModifiers),
    Refreshed(Snapshot),
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

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn visible(&self) -> Vec<&OverviewEntry> {
        let _ = (&self.hidden, &self.seen);
        Vec::new()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

pub fn update(app: App, msg: Msg) -> (App, Vec<Effect>) {
    let _ = msg;
    (app, Vec::new())
}
