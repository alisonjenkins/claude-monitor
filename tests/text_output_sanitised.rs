//! Guard: text derived from untrusted data (a hook payload's cwd and session
//! id, a git branch, a registry name, plan items, notes) must not reach a
//! terminal as a forged row or as an escape sequence, in any text-mode command.
//!
//! One hostile session is run through every command in `CASES`. Each output
//! must hold no control character other than `\n` and `\t`, no C1, no
//! line/paragraph separator, and exactly as many lines as the same command
//! emits for a benign session of the same shape, plus the case's `extra_lines`
//! for the multi-line fields it prints on purpose. Adding a command is one
//! line in `CASES`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;

use claude_monitor::cli::confirm::{confirm_command, Terminal};
use claude_monitor::cli::{run_in, Command, Output, ProjectAction};
use claude_monitor::error::Error;
use claude_monitor::model::{Session, State};
use claude_monitor::store::{legacy, Store, StoreOptions};
use claude_monitor::testing::FakeEnv;
use claude_monitor::watch;
use tempfile::TempDir;

/// A forged row, a screen clear, an OSC title, NEL and LINE SEPARATOR. It has
/// no `/` so it is also a valid file name.
const HOSTILE: &str =
    "\n2026-01-01T00:00:00Z claude abcd1234 finished (was working) y\u{1b}[2J\u{1b}]0;x\u{7}\u{85}\u{2028}end";
const AT: &str = "2026-10-12T00:00:00Z";

struct Rig {
    fake: FakeEnv,
    state: TempDir,
    store: Store,
    session: Session,
    root: PathBuf,
    tail: &'static str,
}

impl Rig {
    /// `hostile` appends [`HOSTILE`] to every untrusted field; `ready` makes
    /// the session resumable (its directory and agent state exist).
    fn new(hostile: bool, ready: bool) -> Self {
        let tail = if hostile { HOSTILE } else { "" };
        let id = format!("abcd1234{tail}");
        let cwd = format!("/work/proj{tail}");
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        fake.vars.set("USER", "tester");
        let state = TempDir::new().unwrap();
        let store = Store::open_in_dir(state.path(), &StoreOptions::viewer()).unwrap();
        let empty = TempDir::new().unwrap();
        legacy::migrate(&store, empty.path(), fake.env.clock.as_ref()).unwrap();

        let mut session = Session::new(&id, "copilot", "agent-1", &cwd, AT);
        session.state = State::Interrupted;
        session.branch = Some(format!("feature{tail}"));
        session.session_name = Some(format!("name{tail}"));
        session.task_description = Some(format!("task{tail}"));
        session.left_off = Some(format!("left off{tail}"));
        session.end_reason = Some(format!("reason{tail}"));
        session.tmux_session = Some(format!("tmux{tail}"));
        session.tmux_window_name = Some(format!("window{tail}"));
        store.upsert_session(&session).unwrap();
        if ready {
            fake.fs.add_dir(cwd.as_str());
            fake.fs.add_dir("/home/u/.copilot/session-state/agent-1");
        }
        let note = if hostile {
            format!("first line{HOSTILE}")
        } else {
            "first line".to_owned()
        };
        store
            .add_note(
                claude_monitor::model::OwnerKind::Session,
                &id,
                &note,
                AT,
                &claude_monitor::model::Author::User {
                    name: "tester".to_owned(),
                },
            )
            .unwrap();
        let root = PathBuf::from(&cwd);
        Self {
            fake,
            state,
            store,
            session,
            root,
            tail,
        }
    }

    fn id(&self) -> String {
        self.session.id.clone()
    }

    /// Two plan files in the session's directory: the second has a hostile
    /// name and hostile task text and id.
    fn with_plans(self) -> Self {
        let tail = self.tail;
        self.fake
            .fs
            .add_file(self.root.join("plan.md"), "- [ ] first\n- [x] second\n");
        self.fake.fs.add_file(
            self.root.join(format!("other{tail}.md")),
            &format!("## Phase 1\n- [ ] T001{tail} do{tail} it\n- [x] T002 done\n"),
        );
        self
    }

    fn run(&self, command: Command) -> String {
        let result = run_in(command, &self.fake.env, Some(self.state.path()));
        render(result)
    }
}

fn render(result: Result<Output, Error>) -> String {
    match result {
        Ok(output) => output.render(false),
        Err(error) => error.render(false),
    }
}

type Run = fn(&Rig) -> String;

struct Case {
    name: &'static str,
    /// Lines the hostile run may have beyond the benign run: multi-line
    /// fields printed on purpose, such as the note's second line.
    extra_lines: usize,
    ready: bool,
    plans: bool,
    run: Run,
}

fn list(rig: &Rig) -> String {
    rig.run(Command::List {
        states: vec![],
        agent: None,
        repo: None,
        project: None,
        text: None,
        section: None,
        include_dismissed: true,
        expand_finished: true,
        expand_unseen: true,
    })
}

fn restore(yes: bool) -> Command {
    Command::Restore {
        all_interrupted: true,
        include_finished: false,
        fresh: false,
        skip: vec![],
        yes,
    }
}

fn watch_lines(rig: &Rig) -> String {
    let mut session = rig.store.get_session(&rig.id()).unwrap().unwrap();
    session.state = State::Idle;
    let sessions = [session];
    let previous = BTreeMap::new();
    let mut text = String::new();
    for (project, state) in watch::notifications(&previous, &sessions, false) {
        let (summary, body) = (
            claude_monitor::notify::SUMMARY,
            claude_monitor::notify::body(&project, state),
        );
        text.push_str(&format!("{summary}\n{body}\n"));
    }
    let mut previous = previous;
    for line in watch::state_changes(&mut previous, &sessions, AT, false) {
        text.push_str(&line);
        text.push('\n');
    }
    text
}

fn boot_check(rig: &Rig) -> String {
    let mut text = rig.run(Command::BootCheck { banner: false });
    for (summary, body) in rig.fake.notifier.sent() {
        text.push_str(&format!("{summary}\n{body}\n"));
    }
    text
}

fn dismiss_prompt(rig: &Rig) -> String {
    let command = Command::Dismiss {
        target: rig.id(),
        yes: false,
    };
    let mut input = Cursor::new(b"n\n".to_vec());
    let mut output = Vec::new();
    let result = confirm_command(
        command.clone(),
        &rig.fake.env,
        Some(rig.state.path()),
        Some(Terminal {
            input: &mut input,
            output: &mut output,
        }),
    );
    let mut text = String::from_utf8(output).unwrap();
    text.push('\n');
    if let Err(error) = result {
        text.push_str(&error.render(false));
    }
    let no_terminal =
        confirm_command(command, &rig.fake.env, Some(rig.state.path()), None).unwrap_err();
    text.push_str(&no_terminal.render(false));
    text
}

const CASES: &[Case] = &[
    Case {
        name: "list",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: list,
    },
    Case {
        name: "show",
        extra_lines: 1,
        ready: true,
        plans: false,
        run: |rig| rig.run(Command::Show { session: rig.id() }),
    },
    Case {
        name: "watch",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: watch_lines,
    },
    Case {
        name: "restore preview, startable",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| rig.run(restore(false)),
    },
    Case {
        name: "restore preview, refused",
        extra_lines: 0,
        ready: false,
        plans: false,
        run: |rig| rig.run(restore(false)),
    },
    Case {
        name: "restore started",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| rig.run(restore(true)),
    },
    Case {
        name: "respawn, startable",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Respawn {
                sessions: vec![rig.id()],
                fresh: false,
                in_dir: None,
                recreate_worktree: false,
            })
        },
    },
    Case {
        name: "respawn, refused",
        extra_lines: 0,
        ready: false,
        plans: false,
        run: |rig| {
            rig.run(Command::Respawn {
                sessions: vec![rig.id()],
                fresh: false,
                in_dir: None,
                recreate_worktree: false,
            })
        },
    },
    Case {
        name: "respawn --fresh in a directory",
        extra_lines: 0,
        ready: false,
        plans: false,
        run: |rig| {
            rig.run(Command::Respawn {
                sessions: vec![rig.id()],
                fresh: true,
                in_dir: Some(PathBuf::from(format!("/work/dir{}", rig.tail))),
                recreate_worktree: false,
            })
        },
    },
    Case {
        name: "progress, several plans",
        extra_lines: 0,
        ready: true,
        plans: true,
        run: |rig| {
            rig.run(Command::ProgressBind {
                owner: rig.id(),
                file: None,
                source: None,
            })
        },
    },
    Case {
        name: "progress bound to the hostile plan",
        extra_lines: 0,
        ready: true,
        plans: true,
        run: |rig| {
            rig.run(Command::ProgressBind {
                owner: rig.id(),
                file: Some(rig.root.join(format!("other{}.md", rig.tail))),
                source: None,
            })
        },
    },
    Case {
        name: "boot-check",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: boot_check,
    },
    Case {
        name: "note",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Note {
                target: rig.id(),
                text: format!("note{}", rig.tail),
            })
        },
    },
    Case {
        name: "link",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Link {
                owner: rig.id(),
                cortex: "42".to_owned(),
                kind: "task".to_owned(),
                doc: None,
            })
        },
    },
    Case {
        name: "unlink",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Unlink {
                owner: rig.id(),
                cortex: "42".to_owned(),
                yes: true,
            })
        },
    },
    Case {
        name: "links check",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| rig.run(Command::LinksCheck),
    },
    Case {
        name: "park",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| rig.run(Command::Park { target: rig.id() }),
    },
    Case {
        name: "dismiss",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Dismiss {
                target: rig.id(),
                yes: true,
            })
        },
    },
    Case {
        name: "dismiss confirmation",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: dismiss_prompt,
    },
    Case {
        name: "done",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Done {
                sessions: vec![rig.id()],
            })
        },
    },
    Case {
        name: "show of an unknown target",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Show {
                session: format!("nope{}", rig.tail),
            })
        },
    },
    Case {
        name: "project add",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            rig.run(Command::Project {
                action: ProjectAction::Add,
                name: format!("proj{}", rig.tail),
                value: None,
            })
        },
    },
    Case {
        name: "export to a file",
        extra_lines: 0,
        ready: true,
        plans: false,
        run: |rig| {
            let out = rig.state.path().join(format!("out{}.jsonl", rig.tail));
            rig.run(Command::ExportCortexiphan {
                project: None,
                include_text: true,
                out: Some(out),
            })
        },
    },
];

fn rig_for(case: &Case, hostile: bool) -> Rig {
    let rig = Rig::new(hostile, case.ready);
    if case.plans {
        rig.with_plans()
    } else {
        rig
    }
}

/// The first character that could drive a terminal, if any.
fn unsafe_char(text: &str) -> Option<char> {
    text.chars().find(|&c| {
        (c.is_control() && c != '\n' && c != '\t')
            || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

#[test]
fn every_text_mode_command_prints_one_hostile_session_inertly() {
    let mut failures = Vec::new();
    for case in CASES {
        let benign = (case.run)(&rig_for(case, false));
        let hostile = (case.run)(&rig_for(case, true));
        if let Some(c) = unsafe_char(&hostile) {
            failures.push(format!(
                "{}: {c:?} reached the terminal: {hostile:?}",
                case.name
            ));
        }
        if hostile.lines().count() != benign.lines().count() + case.extra_lines {
            failures.push(format!(
                "{}: a hostile value forged or dropped a line\n  benign:  {benign:?}\n  hostile: {hostile:?}",
                case.name
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn the_guard_flags_the_raw_hostile_text() {
    // Positive control: the checks above can go red.
    assert!(unsafe_char(HOSTILE).is_some());
    assert!(unsafe_char("plain\ttext\n").is_none());
    assert!(HOSTILE.lines().count() > 1);
}
