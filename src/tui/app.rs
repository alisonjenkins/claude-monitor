//! The dashboard's state machine: `update` is pure and returns the effects
//! (jump to a pane, notify, quit) for the runner to carry out.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use crossterm::event::{KeyCode, KeyModifiers};

use super::bind::{BindChooser, BindOption};
use super::bindings::{action_for, Action};
use super::detail::DetailView;
use super::filter::Filter;
use super::organise::Organise;
use super::projects::{flat, grouped, Part};
use super::restore::{PreviewRow, RestoreResult, RestoreView, Step};
use crate::cli::detail::SessionDetail;
use crate::export::links::Check;
use crate::model::State;
use crate::notify::is_notifiable;
use crate::store::queries::{Flag, Overview, OverviewEntry, OverviewQuery, Section, SectionName};
use crate::store::Store;

const NO_PANE_STATUS: &str = "no tmux pane recorded for this session";
const NO_LINKS_STATUS: &str = "this session has no links to check";
const NOT_FINISHED_STATUS: &str = "only a finished session can be marked done";
const NOT_DONE_STATUS: &str = "that session is not done";
const LINK_USAGE_STATUS: &str = "link takes <node-id> [doc]";

/// What `D`, `U` and `x` ask for on the selected row.
#[derive(Debug, Clone, Copy)]
enum Mark {
    Done,
    Undone,
    Dismiss,
}

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
    /// Reads the overview with the dismissed sessions (the dashboard decides
    /// whether to list them) and the recorded panes; `now` dates the age
    /// column.
    pub fn load(store: &Store, now: &str) -> Result<Self, String> {
        let query = OverviewQuery {
            include_dismissed: true,
            ..OverviewQuery::default()
        };
        let overview = store.overview(&query).map_err(|e| e.to_string())?;
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
    /// The answer to [`Effect::LoadCandidates`]: the rows of the progress
    /// source chooser.
    Candidates {
        id: String,
        options: Vec<BindOption>,
    },
    /// The answer to [`Effect::Summarize`]: stored, or why it was refused.
    Summarized {
        id: String,
        result: Result<(), String>,
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
    /// Find the plan files that could be this session's and answer with
    /// [`Msg::Candidates`].
    LoadCandidates {
        id: String,
    },
    /// Ask the local model for a summary off the UI thread and answer with
    /// [`Msg::Summarized`].
    Summarize {
        id: String,
    },
    /// Change the store as the person asked, then reload.
    Organise(Organise),
    JumpToPane {
        pane: String,
    },
    Notify {
        project: String,
        state: State,
    },
    Quit,
}

/// A single-line input that takes over the keys until Enter or Esc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Typing narrows the list as it goes; Esc clears the text.
    Filter,
    /// A note for this session; Enter saves it.
    Note { id: String, text: String },
    /// A project name for this session (`-` for none); Enter saves it.
    Group { id: String, text: String },
    /// `<node-id> [doc]` to link this session to; Enter saves it.
    Link { id: String, text: String },
}

#[derive(Debug, Clone)]
pub struct App {
    /// Toggled with `p`: rows are laid out under their project.
    grouped: bool,
    filter: Filter,
    prompt: Option<Prompt>,
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
    /// Open with `b`; takes over the keys until closed.
    chooser: Option<BindChooser>,
}

impl App {
    pub fn new() -> Self {
        Self {
            grouped: false,
            filter: Filter::default(),
            prompt: None,
            snapshot: None,
            selected: None,
            hidden: HashSet::new(),
            seen: HashMap::new(),
            status: None,
            unseen_expanded: false,
            restore: None,
            detail: None,
            chooser: None,
        }
    }

    pub fn chooser(&self) -> Option<&BindChooser> {
        self.chooser.as_ref()
    }

    pub fn grouped(&self) -> bool {
        self.grouped
    }

    pub fn filter(&self) -> &Filter {
        &self.filter
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        self.prompt.as_ref()
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

    /// Each section with the entries that are neither hidden nor filtered
    /// out, in display order. A collapsed unseen section shows none, and the
    /// dismissed section is left out unless the filter shows it.
    pub fn sections(&self) -> Vec<(&Section, Vec<&OverviewEntry>)> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .overview
            .sections
            .iter()
            .filter(|section| {
                section.name != SectionName::Dismissed || self.filter.show_dismissed()
            })
            .map(|section| {
                let collapsed = section.name == SectionName::Unseen && !self.unseen_expanded;
                let shown = section
                    .sessions
                    .iter()
                    .filter(|entry| !collapsed && self.is_listed(entry))
                    .collect();
                (section, shown)
            })
            .collect()
    }

    /// Not hidden with `d` and passing the filter.
    fn is_listed(&self, entry: &OverviewEntry) -> bool {
        !self.is_hidden(entry) && self.filter.matches(entry)
    }

    /// The titled runs of rows in display order: the sections, or the
    /// project groups when grouped. A collapsed unseen part counts the rows
    /// it would list.
    pub fn parts(&self) -> Vec<Part<'_>> {
        let sections = self.sections();
        let mut parts = if self.grouped {
            grouped(sections)
        } else {
            flat(sections)
        };
        if !self.unseen_expanded {
            let unseen = self.unseen_listed();
            for part in parts.iter_mut().filter(|p| p.name == SectionName::Unseen) {
                part.total = unseen;
            }
        }
        parts
    }

    fn unseen_listed(&self) -> usize {
        self.snapshot.as_ref().map_or(0, |snapshot| {
            snapshot
                .overview
                .sections
                .iter()
                .filter(|section| section.name == SectionName::Unseen)
                .flat_map(|section| section.sessions.iter())
                .filter(|entry| self.is_listed(entry))
                .count()
        })
    }

    /// Entries in display order, hidden and filtered-out ones left out.
    pub fn visible(&self) -> Vec<&OverviewEntry> {
        self.parts()
            .into_iter()
            .flat_map(|part| part.entries)
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

    fn toggle_grouped(&mut self) {
        let current = self.selected_index();
        self.grouped = !self.grouped;
        self.keep_selection(current.unwrap_or(0));
    }

    /// Applies `change` to the filter and keeps the selection on a row that
    /// is still listed.
    fn change_filter(&mut self, change: impl FnOnce(&mut Filter)) {
        let current = self.selected_index();
        change(&mut self.filter);
        self.keep_selection(current.unwrap_or(0));
    }

    /// The agent kinds in the snapshot, sorted, once each.
    fn agent_kinds(&self) -> Vec<String> {
        let kinds: BTreeSet<&str> = self.snapshot.iter().flat_map(snapshot_agents).collect();
        kinds.into_iter().map(str::to_owned).collect()
    }

    fn selected_entry(&self) -> Option<&OverviewEntry> {
        let selected = self.selected.as_deref()?;
        self.visible()
            .into_iter()
            .find(|entry| entry.id == selected)
    }

    /// The organise effect for the selected row, or a footer message when the
    /// row is not in a state the action applies to.
    fn organise_selected(&mut self, mark: Mark) -> Vec<Effect> {
        let Some((id, section)) = self
            .selected_entry()
            .map(|entry| (entry.id.clone(), entry.section))
        else {
            return Vec::new();
        };
        let action = match (mark, section) {
            (Mark::Done, SectionName::Finished) => Organise::Done { id },
            (Mark::Done, _) => return self.refuse(NOT_FINISHED_STATUS),
            (Mark::Undone, SectionName::Done) => Organise::Undone { id },
            (Mark::Undone, _) => return self.refuse(NOT_DONE_STATUS),
            (Mark::Dismiss, SectionName::Dismissed) => Organise::Undismiss { id },
            (Mark::Dismiss, _) => Organise::Dismiss { id },
        };
        vec![Effect::Organise(action)]
    }

    fn toggle_park(&mut self) -> Vec<Effect> {
        let Some((id, parked)) = self
            .selected_entry()
            .map(|entry| (entry.id.clone(), entry.flags.contains(&Flag::Parked)))
        else {
            return Vec::new();
        };
        let action = if parked {
            Organise::Unpark { id }
        } else {
            Organise::Park { id }
        };
        vec![Effect::Organise(action)]
    }

    fn refuse(&mut self, message: &str) -> Vec<Effect> {
        self.status = Some(message.to_owned());
        Vec::new()
    }

    fn open_note(&mut self) {
        if let Some(id) = self.selected_entry().map(|entry| entry.id.clone()) {
            self.prompt = Some(Prompt::Note {
                id,
                text: String::new(),
            });
        }
    }

    fn open_link(&mut self) {
        if let Some(id) = self.selected_entry().map(|entry| entry.id.clone()) {
            self.prompt = Some(Prompt::Link {
                id,
                text: String::new(),
            });
        }
    }

    fn open_group(&mut self) {
        if let Some(id) = self.selected_entry().map(|entry| entry.id.clone()) {
            self.prompt = Some(Prompt::Group {
                id,
                text: String::new(),
            });
        }
    }

    fn on_prompt_key(&mut self, code: KeyCode) -> Vec<Effect> {
        let Some(prompt) = self.prompt.clone() else {
            return Vec::new();
        };
        match (prompt, code) {
            (Prompt::Filter, KeyCode::Char(c)) => {
                let text = format!("{}{c}", self.filter.text());
                self.change_filter(|filter| filter.set_text(text));
            }
            (Prompt::Filter, KeyCode::Backspace) => {
                let mut text = self.filter.text().to_owned();
                text.pop();
                self.change_filter(|filter| filter.set_text(text));
            }
            (Prompt::Filter, KeyCode::Enter) => self.prompt = None,
            (Prompt::Filter, KeyCode::Esc) => {
                self.prompt = None;
                self.change_filter(|filter| filter.set_text(String::new()));
            }
            (Prompt::Note { id, mut text }, KeyCode::Char(c)) => {
                text.push(c);
                self.prompt = Some(Prompt::Note { id, text });
            }
            (Prompt::Note { id, mut text }, KeyCode::Backspace) => {
                text.pop();
                self.prompt = Some(Prompt::Note { id, text });
            }
            (Prompt::Note { id, text }, KeyCode::Enter) => {
                self.prompt = None;
                let text = text.trim();
                if !text.is_empty() {
                    return vec![Effect::Organise(Organise::Note {
                        id,
                        text: text.to_owned(),
                    })];
                }
            }
            (Prompt::Note { .. }, KeyCode::Esc) => self.prompt = None,
            (Prompt::Group { id, mut text }, KeyCode::Char(c)) => {
                text.push(c);
                self.prompt = Some(Prompt::Group { id, text });
            }
            (Prompt::Group { id, mut text }, KeyCode::Backspace) => {
                text.pop();
                self.prompt = Some(Prompt::Group { id, text });
            }
            (Prompt::Group { id, text }, KeyCode::Enter) => {
                self.prompt = None;
                let project = text.trim();
                if !project.is_empty() {
                    return vec![Effect::Organise(Organise::Group {
                        id,
                        project: project.to_owned(),
                    })];
                }
            }
            (Prompt::Group { .. }, KeyCode::Esc) => self.prompt = None,
            (Prompt::Link { id, mut text }, KeyCode::Char(c)) => {
                text.push(c);
                self.prompt = Some(Prompt::Link { id, text });
            }
            (Prompt::Link { id, mut text }, KeyCode::Backspace) => {
                text.pop();
                self.prompt = Some(Prompt::Link { id, text });
            }
            (Prompt::Link { id, text }, KeyCode::Enter) => {
                self.prompt = None;
                let words: Vec<&str> = text.split_whitespace().collect();
                match words.as_slice() {
                    [] => {}
                    [node_id] | [node_id, _] => {
                        return vec![Effect::Organise(Organise::Link {
                            id,
                            node_id: (*node_id).to_owned(),
                            doc: words.get(1).map(|doc| (*doc).to_owned()),
                        })];
                    }
                    _ => return self.refuse(LINK_USAGE_STATUS),
                }
            }
            (Prompt::Link { .. }, KeyCode::Esc) => self.prompt = None,
            _ => {}
        }
        Vec::new()
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
        if self.chooser.is_some() {
            return self.on_chooser_key(code);
        }
        if self.detail.is_some() {
            return self.on_detail_key(code);
        }
        if self.prompt.is_some() {
            return self.on_prompt_key(code);
        }
        match code {
            KeyCode::Char(c) => {
                if let Some(action) = action_for(c) {
                    return self.perform(action);
                }
            }
            KeyCode::Esc => return vec![Effect::Quit],
            KeyCode::Down => self.move_selection(true),
            KeyCode::Up => self.move_selection(false),
            KeyCode::Enter => return self.jump(),
            _ => {}
        }
        Vec::new()
    }

    fn perform(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::ToggleGrouped => self.toggle_grouped(),
            Action::Filter => self.prompt = Some(Prompt::Filter),
            Action::CycleState => self.change_filter(Filter::cycle_state),
            Action::CycleAgent => {
                let kinds = self.agent_kinds();
                self.change_filter(|filter| filter.cycle_agent(&kinds));
            }
            Action::ToggleDismissed => self.change_filter(Filter::toggle_dismissed),
            Action::Done => return self.organise_selected(Mark::Done),
            Action::Undone => return self.organise_selected(Mark::Undone),
            Action::Dismiss => return self.organise_selected(Mark::Dismiss),
            Action::Note => self.open_note(),
            Action::TogglePark => return self.toggle_park(),
            Action::Group => self.open_group(),
            Action::Link => self.open_link(),
            Action::Summarize => return self.summarize_selected(),
            Action::BindProgress => return self.load_candidates(),
            Action::Detail => return self.open_detail(),
            Action::Links => return self.open_detail_and_check_links(),
            Action::Restore => self.open_restore(),
            Action::Quit => return vec![Effect::Quit],
            Action::Next => self.move_selection(true),
            Action::Previous => self.move_selection(false),
            Action::Hide => self.hide_selected(),
            Action::ToggleUnseen => self.toggle_unseen(),
        }
        Vec::new()
    }

    fn on_refreshed(&mut self, snapshot: Snapshot) -> Vec<Effect> {
        let entries: Vec<&OverviewEntry> = snapshot
            .overview
            .sections
            .iter()
            .filter(|section| section.name != SectionName::Dismissed)
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

    /// Opens the detail page, where the generated text lands, and asks for
    /// the summary. Only this key ever does.
    fn summarize_selected(&mut self) -> Vec<Effect> {
        let mut effects = self.open_detail();
        if let Some(open) = &self.detail {
            effects.push(Effect::Summarize {
                id: open.id().to_owned(),
            });
        }
        effects
    }

    fn load_candidates(&self) -> Vec<Effect> {
        self.selected_entry()
            .map(|entry| Effect::LoadCandidates {
                id: entry.id.clone(),
            })
            .into_iter()
            .collect()
    }

    /// Opens the chooser, unless the person has moved to another row since
    /// asking.
    fn on_candidates(&mut self, id: &str, options: Vec<BindOption>) {
        if self.selected.as_deref() == Some(id) {
            self.chooser = Some(BindChooser::new(id, options));
        }
    }

    fn on_chooser_key(&mut self, code: KeyCode) -> Vec<Effect> {
        let Some(chooser) = self.chooser.as_mut() else {
            return Vec::new();
        };
        match code {
            KeyCode::Char('j') | KeyCode::Down => chooser.step(true),
            KeyCode::Char('k') | KeyCode::Up => chooser.step(false),
            KeyCode::Esc => self.chooser = None,
            KeyCode::Enter => {
                let id = chooser.id().to_owned();
                let path = chooser.highlighted().and_then(|option| option.path.clone());
                let chosen = chooser.highlighted().is_some();
                self.chooser = None;
                if chosen {
                    return vec![Effect::Organise(Organise::ProgressBind { id, path })];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn on_summarized(&mut self, id: &str, result: Result<(), String>) -> Vec<Effect> {
        match result {
            Ok(()) if self.detail.as_ref().is_some_and(|open| open.id() == id) => {
                vec![Effect::LoadDetail { id: id.to_owned() }]
            }
            Ok(()) => Vec::new(),
            Err(message) => {
                self.status = Some(message);
                Vec::new()
            }
        }
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

fn snapshot_agents(snapshot: &Snapshot) -> impl Iterator<Item = &str> {
    snapshot
        .overview
        .sections
        .iter()
        .flat_map(|section| section.sessions.iter())
        .map(|entry| entry.agent_kind.as_str())
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
        Msg::Summarized { id, result } => app.on_summarized(&id, result),
        Msg::Candidates { id, options } => {
            app.on_candidates(&id, options);
            Vec::new()
        }
    };
    (app, effects)
}
