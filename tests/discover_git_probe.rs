#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

//! A session no hook has seen has no git facts but the transcript's, which can
//! be stale. Discovery reads the live `.git/HEAD` for those rows.

use std::fs;
use std::path::Path;

use claude_monitor::agents::discover_store::store_report;
use claude_monitor::agents::{ScanReport, SessionStub};
use claude_monitor::env::Env;
use claude_monitor::hook::{FsGit, NoGit};
use claude_monitor::model::{Session, State};
use claude_monitor::store::{Store, StoreOptions};
use tempfile::TempDir;

const KIND: &str = "claude";
const SID: &str = "s-1";

struct Fx {
    dir: TempDir,
    store: Store,
    env: Env,
}

impl Fx {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store =
            Store::open_in_dir(&dir.path().join("state"), &StoreOptions::default()).unwrap();
        Self {
            dir,
            store,
            env: Env::real(),
        }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }

    /// `<rel>/.git/HEAD` containing `head`; returns the work tree.
    fn repo(&self, rel: &str, head: &str) -> std::path::PathBuf {
        let root = self.path(rel);
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), head).unwrap();
        root
    }

    /// A linked worktree `<rel>` of `main`, with its own HEAD.
    fn linked_worktree(
        &self,
        main: &Path,
        name: &str,
        rel: &str,
        head: &str,
    ) -> std::path::PathBuf {
        let admin = main.join(".git/worktrees").join(name);
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("HEAD"), head).unwrap();
        let tree = self.path(rel);
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        tree
    }

    fn stub(cwd: &Path, branch: Option<&str>) -> SessionStub {
        SessionStub {
            agent_session_id: SID.to_owned(),
            cwd: cwd.to_str().unwrap().to_owned(),
            repo_root: None,
            branch: branch.map(str::to_owned),
            name: None,
            started_at: "2026-10-12T09:00:00Z".to_owned(),
            last_activity_at: "2026-10-12T09:30:00Z".to_owned(),
            first_prompt: None,
        }
    }

    /// Stores the transcript view of a session without reading git.
    fn discover_without_git(&self, cwd: &Path, branch: Option<&str>) {
        let report = ScanReport {
            stubs: vec![Self::stub(cwd, branch)],
            degraded: 0,
        };
        store_report(&self.env, &self.store, KIND, &report, &NoGit).unwrap();
    }

    /// A discovery pass that sees the same transcript and reads real git.
    fn rediscover(&self, cwd: &Path, branch: Option<&str>) {
        let report = ScanReport {
            stubs: vec![Self::stub(cwd, branch)],
            degraded: 0,
        };
        store_report(&self.env, &self.store, KIND, &report, &FsGit).unwrap();
    }

    fn row(&self) -> Session {
        self.store
            .get_session_by_agent_id(KIND, SID)
            .unwrap()
            .unwrap()
    }
}

#[test]
fn a_stale_transcript_branch_is_corrected_from_head() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "ref: refs/heads/feature-x\n");
    fx.discover_without_git(&repo, Some("main"));
    assert_eq!(fx.row().branch.as_deref(), Some("main"));

    fx.rediscover(&repo, Some("main"));

    let row = fx.row();
    assert_eq!(row.branch.as_deref(), Some("feature-x"));
    assert_eq!(row.repo_root.as_deref(), repo.to_str());
    assert_eq!(row.state, State::Unknown);
}

#[test]
fn a_missing_branch_and_repo_root_are_filled() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "ref: refs/heads/main\n");
    fx.discover_without_git(&repo, None);

    fx.rediscover(&repo, None);

    let row = fx.row();
    assert_eq!(row.branch.as_deref(), Some("main"));
    assert_eq!(row.repo_root.as_deref(), repo.to_str());
}

#[test]
fn a_linked_worktree_reports_its_own_head() {
    let fx = Fx::new();
    let main = fx.repo("main-checkout", "ref: refs/heads/main\n");
    let tree = fx.linked_worktree(
        &main,
        "wt",
        "wt",
        "ref: refs/heads/worktree-aquaero-ctl-spec\n",
    );
    fx.discover_without_git(&tree, Some("main"));

    fx.rediscover(&tree, Some("main"));

    let row = fx.row();
    assert_eq!(row.branch.as_deref(), Some("worktree-aquaero-ctl-spec"));
    assert_eq!(row.repo_root.as_deref(), tree.to_str());
}

#[test]
fn a_detached_head_clears_the_transcript_branch() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "0123456789abcdef0123456789abcdef01234567\n");
    fx.discover_without_git(&repo, Some("main"));

    fx.rediscover(&repo, Some("main"));

    let row = fx.row();
    assert_eq!(row.branch, None);
    assert_eq!(row.repo_root.as_deref(), repo.to_str());
}

#[test]
fn a_cwd_outside_git_is_left_alone() {
    let fx = Fx::new();
    let plain = fx.path("plain");
    fs::create_dir_all(&plain).unwrap();
    fx.discover_without_git(&plain, Some("main"));
    let before = fx.row();

    fx.rediscover(&plain, Some("main"));

    assert_eq!(fx.row(), before);
}

#[test]
fn a_missing_cwd_inside_a_repo_is_left_alone() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "ref: refs/heads/feature-x\n");
    let gone = repo.join("deleted/sub");
    fx.discover_without_git(&gone, Some("main"));
    let before = fx.row();

    fx.rediscover(&gone, Some("main"));

    assert_eq!(fx.row(), before);
}

#[test]
fn a_row_a_hook_already_gave_git_facts_is_not_overwritten() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "ref: refs/heads/feature-x\n");
    fx.discover_without_git(&repo, Some("main"));
    let mut hooked = fx.row();
    hooked.repo_root = Some("/hook/root".to_owned());
    hooked.branch = Some("hook-branch".to_owned());
    fx.store.upsert_session(&hooked).unwrap();

    fx.rediscover(&repo, Some("main"));

    assert_eq!(fx.row(), hooked);
}

#[test]
fn a_second_pass_changes_nothing_and_appends_no_event() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "ref: refs/heads/feature-x\n");
    fx.discover_without_git(&repo, Some("main"));
    fx.rediscover(&repo, Some("main"));
    let after_first = fx.row();
    let events = fx.store.events_since(0, 100).unwrap().len();

    fx.rediscover(&repo, Some("main"));

    assert_eq!(fx.row(), after_first);
    assert_eq!(fx.store.events_since(0, 100).unwrap().len(), events);
}

#[test]
fn a_detached_head_stays_detached_across_passes() {
    let fx = Fx::new();
    let repo = fx.repo("repo", "0123456789abcdef0123456789abcdef01234567\n");
    fx.discover_without_git(&repo, Some("main"));
    fx.rediscover(&repo, Some("main"));
    let after_first = fx.row();

    fx.rediscover(&repo, Some("main"));

    assert_eq!(fx.row(), after_first);
    assert_eq!(fx.row().branch, None);
}
