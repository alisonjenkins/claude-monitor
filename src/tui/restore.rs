//! The restore view (`R`): pick which sessions to bring back, see what would
//! start, confirm, then watch each start. The view-model is pure; the runner
//! plans and starts through the effects it returns.

use crossterm::event::KeyCode;

use crate::env::Env;
use crate::respawn::{plan_batch, start_one, Item, PlanFlags, SPAWN_FAILED};
use crate::store::queries::{Overview, OverviewEntry, SectionName};
use crate::store::Store;

const NOTHING_SELECTED: &str = "nothing selected";
const NOTHING_CAN_START: &str = "none of the selected sessions can start";
const STILL_STARTING: &str = "still starting";

/// Plan `ids` for the confirm screen. Reads the filesystem and process table;
/// starts and writes nothing.
pub fn preview_restore(store: &Store, env: &Env, ids: &[String]) -> Vec<PreviewRow> {
    match plan_batch(store, env, ids, &PlanFlags::default()) {
        Ok(planned) => planned
            .into_iter()
            .map(|(session, plan)| match plan {
                Ok(plan) => PreviewRow::Start {
                    session_id: session.id,
                    mode: plan.mode.as_str().to_owned(),
                    cwd: plan.launch.cwd.display().to_string(),
                    notices: plan.notices,
                },
                Err(refusal) => PreviewRow::Refuse {
                    session_id: session.id,
                    code: refusal.code.as_str().to_owned(),
                    message: refusal.message,
                    alternatives: refusal
                        .alternatives
                        .into_iter()
                        .map(|a| (a.description, a.command))
                        .collect(),
                },
            })
            .collect(),
        Err(error) => ids
            .iter()
            .map(|id| PreviewRow::Refuse {
                session_id: id.clone(),
                code: SPAWN_FAILED.to_owned(),
                message: error.to_string(),
                alternatives: Vec::new(),
            })
            .collect(),
    }
}

/// Start `ids` one after another, calling `emit` as each finishes. A failure
/// to start one does not stop the others.
pub fn run_restore(store: &Store, env: &Env, ids: &[String], emit: &mut dyn FnMut(RestoreResult)) {
    let planned = match plan_batch(store, env, ids, &PlanFlags::default()) {
        Ok(planned) => planned,
        Err(error) => {
            for id in ids {
                emit(RestoreResult::Refused {
                    session_id: id.clone(),
                    code: SPAWN_FAILED.to_owned(),
                    message: error.to_string(),
                });
            }
            return;
        }
    };
    let now = env.clock.now();
    for (session, plan) in planned {
        emit(match start_one(store, env, &session, plan, &now) {
            Item::Started(item) => RestoreResult::Started {
                session_id: item.started.session_id,
                pane: item.started.pane_id,
            },
            Item::Refused(refused) => RestoreResult::Refused {
                session_id: refused.session_id,
                code: refused.code.to_owned(),
                message: refused.message,
            },
        });
    }
}

/// Where a row came from; decides whether it starts selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Interrupted,
    Finished,
    Unseen,
}

#[derive(Debug, Clone)]
pub struct RestoreRow {
    pub entry: OverviewEntry,
    pub group: Group,
    pub selected: bool,
}

/// What the confirm screen says will happen to one selected session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewRow {
    Start {
        session_id: String,
        mode: String,
        cwd: String,
        notices: Vec<String>,
    },
    Refuse {
        session_id: String,
        code: String,
        message: String,
        /// `(description, command)` pairs, never taken automatically.
        alternatives: Vec<(String, String)>,
    },
}

/// What became of one session once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreResult {
    Started {
        session_id: String,
        pane: String,
    },
    Refused {
        session_id: String,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Stage {
    /// Choosing sessions.
    #[default]
    Select,
    /// Waiting for the plans of the selection.
    Previewing,
    /// One screen: what will start and what is refused. Enter starts.
    Confirm(Vec<PreviewRow>),
    /// Starting; results arrive one by one.
    Running {
        total: usize,
        results: Vec<RestoreResult>,
    },
    Finished(Vec<RestoreResult>),
}

/// What a key did to the view beyond changing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Stay,
    Close,
    /// Plan these sessions for the confirm screen.
    Preview(Vec<String>),
    /// Start these sessions now.
    Start(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct RestoreView {
    rows: Vec<RestoreRow>,
    cursor: usize,
    stage: Stage,
    notice: Option<String>,
}

impl RestoreView {
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// A one-line reminder for the footer, such as "nothing selected".
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn on_key(&mut self, code: KeyCode) -> Step {
        self.notice = None;
        match std::mem::take(&mut self.stage) {
            Stage::Select => self.select_key(code),
            Stage::Previewing => {
                self.stage = if code == KeyCode::Esc {
                    Stage::Select
                } else {
                    Stage::Previewing
                };
                Step::Stay
            }
            Stage::Confirm(rows) => self.confirm_key(code, rows),
            running @ Stage::Running { .. } => {
                self.stage = running;
                self.notice = Some(STILL_STARTING.to_owned());
                Step::Stay
            }
            Stage::Finished(results) => {
                if matches!(code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    Step::Close
                } else {
                    self.stage = Stage::Finished(results);
                    Step::Stay
                }
            }
        }
    }

    fn select_key(&mut self, code: KeyCode) -> Step {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return Step::Close,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(true),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(false),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('a') => self.select_all_interrupted(),
            KeyCode::Char('n') => self.select_none(),
            KeyCode::Enter => {
                let ids = self.selected_ids();
                if ids.is_empty() {
                    self.notice = Some(NOTHING_SELECTED.to_owned());
                } else {
                    self.stage = Stage::Previewing;
                    return Step::Preview(ids);
                }
            }
            _ => {}
        }
        Step::Stay
    }

    fn confirm_key(&mut self, code: KeyCode, rows: Vec<PreviewRow>) -> Step {
        match code {
            KeyCode::Esc => {
                self.stage = Stage::Select;
                Step::Stay
            }
            KeyCode::Enter => {
                let ids: Vec<String> = rows
                    .iter()
                    .filter_map(|row| match row {
                        PreviewRow::Start { session_id, .. } => Some(session_id.clone()),
                        PreviewRow::Refuse { .. } => None,
                    })
                    .collect();
                if ids.is_empty() {
                    self.notice = Some(NOTHING_CAN_START.to_owned());
                    self.stage = Stage::Confirm(rows);
                    return Step::Stay;
                }
                let refused: Vec<RestoreResult> = rows
                    .iter()
                    .filter_map(|row| match row {
                        PreviewRow::Refuse {
                            session_id,
                            code,
                            message,
                            ..
                        } => Some(RestoreResult::Refused {
                            session_id: session_id.clone(),
                            code: code.clone(),
                            message: message.clone(),
                        }),
                        PreviewRow::Start { .. } => None,
                    })
                    .collect();
                self.stage = Stage::Running {
                    total: rows.len(),
                    results: refused,
                };
                Step::Start(ids)
            }
            _ => {
                self.stage = Stage::Confirm(rows);
                Step::Stay
            }
        }
    }

    /// The plans arrived: show them, unless the user backed out meanwhile.
    pub fn on_preview(&mut self, rows: Vec<PreviewRow>) {
        if self.stage == Stage::Previewing {
            self.stage = Stage::Confirm(rows);
        }
    }

    /// One session finished starting; the last one completes the view.
    pub fn on_result(&mut self, result: RestoreResult) {
        if let Stage::Running { total, results } = &mut self.stage {
            results.push(result);
            if results.len() >= *total {
                self.stage = Stage::Finished(std::mem::take(results));
            }
        }
    }

    /// Rows for the interrupted, finished and unseen sections. Dismissed,
    /// done and active sessions are not offered.
    pub fn from_overview(overview: &Overview) -> Self {
        let rows = overview
            .sections
            .iter()
            .filter_map(|section| {
                let group = match section.name {
                    SectionName::Interrupted => Group::Interrupted,
                    SectionName::Finished => Group::Finished,
                    SectionName::Unseen => Group::Unseen,
                    _ => return None,
                };
                Some((group, &section.sessions))
            })
            .flat_map(|(group, sessions)| {
                sessions.iter().map(move |entry| RestoreRow {
                    entry: entry.clone(),
                    group,
                    selected: group == Group::Interrupted,
                })
            })
            .collect();
        Self {
            rows,
            cursor: 0,
            ..Self::default()
        }
    }

    pub fn rows(&self) -> &[RestoreRow] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn move_cursor(&mut self, forward: bool) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = if forward {
            self.cursor.saturating_add(1).min(last)
        } else {
            self.cursor.saturating_sub(1)
        };
    }

    /// Flip the selection of the row under the cursor. Only the selection
    /// changes: the session stays interrupted and is not dismissed.
    pub fn toggle(&mut self) {
        if let Some(row) = self.rows.get_mut(self.cursor) {
            row.selected = !row.selected;
        }
    }

    pub fn select_all_interrupted(&mut self) {
        for row in &mut self.rows {
            if row.group == Group::Interrupted {
                row.selected = true;
            }
        }
    }

    pub fn select_none(&mut self) {
        for row in &mut self.rows {
            row.selected = false;
        }
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.selected)
            .map(|row| row.entry.id.clone())
            .collect()
    }
}
