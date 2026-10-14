#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The `notify` watch on bound plan files (T089): a wake when a bound file
//! changes, none for its neighbours, and new bindings picked up while it runs.
//! Real-OS tests, with the generous timeouts `tests/feed.rs` uses.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use claude_monitor::feed::{BoundFilesSource, EventSource};
use claude_monitor::model::OwnerKind;
use claude_monitor::progress::{Source, SourceKind};
use claude_monitor::store::progress::ChosenBy;
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const T0: &str = "2026-10-12T10:00:00Z";
// FSEvents batches events behind a latency window, so macOS gets a longer bound.
const REAL_OS_WAKE_TIMEOUT: Duration = if cfg!(target_os = "macos") {
    Duration::from_secs(5)
} else {
    Duration::from_secs(2)
};
const QUIET: Duration = Duration::from_millis(500);
const SETTLE: Duration = Duration::from_millis(300);
const MAX_REPLAYED_WAKES: u32 = 5;
const PICK_UP_DEADLINE: Duration = Duration::from_secs(6);

struct Rig {
    state: TempDir,
    tree: TempDir,
    store: Store,
}

fn rig() -> Rig {
    let state = TempDir::new().unwrap();
    let store = Store::open_in_dir(state.path(), &StoreOptions::default()).unwrap();
    Rig {
        state,
        tree: TempDir::new().unwrap(),
        store,
    }
}

impl Rig {
    fn file(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.tree.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn bind(&self, owner: &str, path: &Path) {
        let source = Source {
            kind: SourceKind::MarkdownChecklist,
            path: Some(path.display().to_string()),
        };
        self.store
            .bind_progress(OwnerKind::Session, owner, &source, ChosenBy::Auto, T0)
            .unwrap();
    }

    fn source(&self) -> BoundFilesSource {
        BoundFilesSource::watch(self.store.path()).unwrap()
    }
}

// FSEvents may replay the creation of a file made just before the watch
// started. A spurious wake only triggers a re-read, so drain those and assert
// on what follows. A no-op on inotify.
fn settle(source: &mut BoundFilesSource) {
    let mut replayed: u32 = 0;
    while source.wait(SETTLE) {
        replayed = replayed.saturating_add(1);
        assert!(replayed < MAX_REPLAYED_WAKES, "the watch never settled");
    }
}

#[test]
fn nothing_changes_so_nothing_wakes() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let mut source = rig.source();
    settle(&mut source);

    assert!(!source.wait(QUIET));
}

#[test]
fn editing_a_bound_file_wakes() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let mut source = rig.source();

    fs::write(&plan, "- [x] one\n").unwrap();

    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "no wake within 2s");
}

#[test]
fn an_atomic_replace_wakes() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let mut source = rig.source();

    let scratch = rig.file(".todo.md.swp", "- [x] one\n");
    fs::rename(&scratch, &plan).unwrap();

    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "no wake within 2s");
}

#[test]
fn removing_a_bound_file_wakes() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let mut source = rig.source();

    fs::remove_file(&plan).unwrap();

    assert!(source.wait(REAL_OS_WAKE_TIMEOUT), "no wake within 2s");
}

#[test]
fn a_neighbouring_file_does_not_wake() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let mut source = rig.source();
    settle(&mut source);

    rig.file("notes.txt", "unrelated\n");

    assert!(!source.wait(QUIET), "woke for a file nobody bound");
}

#[test]
fn a_binding_made_while_running_is_picked_up() {
    let rig = rig();
    let first = rig.file("a/todo.md", "- [ ] one\n");
    rig.bind("s1", &first);
    let mut source = rig.source();
    let later = rig.file("b/todo.md", "- [ ] two\n");
    let writer = Store::open_in_dir(rig.state.path(), &StoreOptions::default()).unwrap();
    writer
        .bind_progress(
            OwnerKind::Session,
            "s2",
            &Source {
                kind: SourceKind::MarkdownChecklist,
                path: Some(later.display().to_string()),
            },
            ChosenBy::User,
            T0,
        )
        .unwrap();

    // The watcher re-reads the bindings on a timer, and FSEvents can wake it
    // early by replaying the first plan file's creation, so a wake alone does
    // not say the new binding is in: wait until it is listed.
    let started = Instant::now();
    while started.elapsed() < PICK_UP_DEADLINE && !source.watched().contains(&later) {
        fs::write(&later, "- [x] two\n").unwrap();
        source.wait(Duration::from_millis(300));
    }
    assert!(
        source.watched().contains(&later),
        "the new binding was never watched"
    );

    settle(&mut source);
    fs::write(&later, "- [ ] two\n").unwrap();
    assert!(
        source.wait(REAL_OS_WAKE_TIMEOUT),
        "an edit to the new file did not wake"
    );
}

#[test]
fn an_unwatchable_path_is_skipped_with_a_warning() {
    let rig = rig();
    let plan = rig.file("todo.md", "- [ ] one\n");
    rig.bind("s1", &plan);
    let gone = rig.tree.path().join("missing-dir/todo.md");
    rig.bind("s2", &gone);

    let mut source = rig.source();

    assert_eq!(source.warnings().len(), 1, "{:?}", source.warnings());
    assert!(source.warnings()[0].contains("missing-dir"));
    assert_eq!(source.watched(), vec![plan.clone()]);
    fs::write(&plan, "- [x] one\n").unwrap();
    assert!(
        source.wait(REAL_OS_WAKE_TIMEOUT),
        "the good file still wakes"
    );
}

#[test]
fn an_agent_todo_binding_has_no_file_to_watch() {
    let rig = rig();
    rig.store
        .bind_progress(
            OwnerKind::Session,
            "s1",
            &Source {
                kind: SourceKind::AgentTodo,
                path: None,
            },
            ChosenBy::User,
            T0,
        )
        .unwrap();

    let source = rig.source();

    assert!(source.watched().is_empty());
    assert!(source.warnings().is_empty());
}
