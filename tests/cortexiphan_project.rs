//! FR-029: cortexiphan's own worktrees and spec task lists are tracked as an
//! ordinary project, with no special-casing. The fixture mirrors its layout
//! (`.claude/worktrees/<name>` checkouts, spec-kit `tasks.md`); the proof is
//! that renaming it changes nothing but the names, and that no logic module
//! mentions cortexiphan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod export_support;

use std::path::{Path, PathBuf};

use claude_monitor::model::State;
use claude_monitor::progress::speckit;
use export_support::{by_id, of_kind, options, parse, system, user, Fixture};

const TASKS_PATH: &str = "specs/012-resilient-client/tasks.md";

fn tasks_md() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/export/cortexiphan-tasks.md");
    std::fs::read_to_string(path).unwrap()
}

/// A project shaped like cortexiphan's own: a checkout plus two Claude
/// worktrees under `.claude/worktrees`, and one spec's `tasks.md` as its plan.
fn repo_project(repo: &str) -> Fixture {
    let root = format!("/home/ali/git/personal/{repo}");
    let text = tasks_md().replace("cortexiphan", repo);
    let progress = speckit::parse(&text, Some(TASKS_PATH)).unwrap();

    let fix = Fixture::new();
    fix.project("proj-1", repo, "active", Some("bind-1"));
    fix.binding("bind-1", "proj-1", TASKS_PATH);
    for item in &progress.items {
        let status = serde_json::to_value(item.status).unwrap();
        fix.step(
            "bind-1",
            item.ordinal,
            item.external_id.as_deref(),
            &item.text,
            status.as_str().unwrap(),
        );
    }
    fix.session("main", Some("proj-1"), |s| {
        s.repo_root = Some(root.clone());
        s.cwd = root.clone();
        s.branch = Some("main".to_owned());
        s.state = State::Idle;
    });
    for name in ["happy-otter", "calm-fox"] {
        let worktree = format!("{root}/.claude/worktrees/{name}");
        fix.session(name, Some("proj-1"), |s| {
            s.repo_root = Some(worktree.clone());
            s.cwd = worktree.clone();
            s.branch = Some(format!("worktree-{name}"));
            s.state = State::Interrupted;
        });
    }
    fix.event("2026-10-12T10:00:01Z", "project", "proj-1", &user("alison"));
    fix.event(
        "2026-10-12T10:05:00Z",
        "progress_binding",
        "bind-1",
        &system("reconcile"),
    );
    fix
}

#[test]
fn the_spec_task_list_exports_one_task_per_step_with_its_plan_id() {
    let records = repo_project("cortexiphan").records(&options());
    let progress = speckit::parse(&tasks_md(), Some(TASKS_PATH)).unwrap();
    assert!(progress.total >= 10, "fixture should be a real-sized list");

    let tasks = of_kind(&records, "task");
    assert_eq!(tasks.len(), progress.items.len());
    for item in &progress.items {
        let id = format!("proj-1:{}", item.external_id.as_deref().unwrap());
        let task = by_id(&records, "task", &id);
        let title = task["node"]["title"].as_str().unwrap();
        assert!(
            title.starts_with(item.external_id.as_deref().unwrap()),
            "{title}"
        );
        assert_eq!(task["node"]["source"], TASKS_PATH);
        assert_eq!(task["node"]["part_of"], "proj-1");
    }
    let done = tasks
        .iter()
        .filter(|t| t["node"]["status"] == "done")
        .count();
    assert_eq!(done as u32, progress.done);
}

#[test]
fn its_worktrees_export_as_sessions_and_clones_of_one_project() {
    let records = repo_project("cortexiphan").records(&options());
    assert_eq!(of_kind(&records, "project").len(), 1);
    assert_eq!(of_kind(&records, "session").len(), 3);

    let session = &by_id(&records, "session", "happy-otter")["node"];
    assert_eq!(session["branch"], "worktree-happy-otter");
    assert_eq!(session["project"], "proj-1");
    assert_eq!(
        session["cwd"],
        "/home/ali/git/personal/cortexiphan/.claude/worktrees/happy-otter"
    );

    let repos = by_id(&records, "project", "proj-1")["node"]["repos"].clone();
    let clone_paths: Vec<_> = repos
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["clones"].as_array().unwrap().clone())
        .map(|c| c["path"].as_str().unwrap().to_owned())
        .collect();
    for path in [
        "/home/ali/git/personal/cortexiphan",
        "/home/ali/git/personal/cortexiphan/.claude/worktrees/happy-otter",
        "/home/ali/git/personal/cortexiphan/.claude/worktrees/calm-fox",
    ] {
        assert!(
            clone_paths.contains(&path.to_owned()),
            "{path} missing from {clone_paths:?}"
        );
    }
}

#[test]
fn renaming_the_repository_changes_nothing_but_the_names() {
    // "dove-repo" sorts between the worktree names, as "cortexiphan" does, so
    // the sorted repository list keeps its order.
    let as_cortexiphan = repo_project("cortexiphan").export(&options());
    let as_other = repo_project("dove-repo").export(&options());
    assert_ne!(as_cortexiphan, as_other);
    let renamed = as_cortexiphan.replace("cortexiphan", "dove-repo");
    let differing: Vec<_> = renamed
        .lines()
        .zip(as_other.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert!(
        differing.is_empty() && renamed.len() == as_other.len(),
        "the export depends on the repository's name beyond echoing it: {differing:?}"
    );
    assert_eq!(parse(&as_other).len(), parse(&as_cortexiphan).len());
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The export and the commands that name it (`export cortexiphan`, `link`,
/// `links check`) and a links view may say cortexiphan: that is the target, not
/// a project. Everything else is logic that must treat all projects alike.
fn may_name_cortexiphan(relative: &Path) -> bool {
    relative.starts_with("src/export")
        || relative.starts_with("src/cli")
        || relative.file_name().is_some_and(|name| name == "links.rs")
}

#[test]
fn no_logic_module_mentions_cortexiphan() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(files.len() > 20, "found only {} source files", files.len());

    let mut offenders = Vec::new();
    for file in files {
        let relative = file.strip_prefix(&root).unwrap();
        if may_name_cortexiphan(relative) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in text.lines().enumerate() {
            if line.to_lowercase().contains("cortexiphan") {
                offenders.push(format!(
                    "{}:{}: {}",
                    relative.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "special-casing cortexiphan breaks FR-029:\n{}",
        offenders.join("\n")
    );
}
