//! Every command a refusal prints as an alternative is one clean line that a
//! shell reads back as the very words the planner meant, whatever the session
//! id, directory, repository or branch hold.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::process::Command;

use claude_monitor::model::{Session, State};
use claude_monitor::respawn::{plan_respawn, Options, RefusalCode};
use claude_monitor::testing::FakeEnv;

const AT: &str = "2026-10-12T00:00:00Z";
const ID: &str = "abcd1234\n\u{1b}[2J; rm -x";
const CWD: &str = "/w/my wt'q\"";
const REPO: &str = "/repos/my proj'x$(touch y)";
const BRANCH: &str = "fe at'ure";

/// What bash makes of `line` once `program` (its first word) is dropped; the
/// whole line must also pass `bash -n`.
fn bash_words(line: &str, program: &str) -> Vec<String> {
    let rest = line
        .strip_prefix(&format!("{program} "))
        .expect("the line starts with its program");
    let syntax = Command::new("bash")
        .args(["-n", "-c", line])
        .output()
        .unwrap();
    assert!(syntax.status.success(), "bash -n rejects {line:?}");
    let script = format!("set -- {rest}\nprintf '%s\\0' \"$@\"");
    let out = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(out.status.success(), "bash cannot split {line:?}");
    let mut words: Vec<String> = out
        .stdout
        .split(|&b| b == 0)
        .map(|w| String::from_utf8(w.to_vec()).unwrap())
        .collect();
    assert_eq!(words.pop().as_deref(), Some(""), "{line:?}");
    words
}

struct Case {
    name: &'static str,
    cwd_exists: bool,
    branch_in_repo: bool,
    branch_recorded: bool,
    code: RefusalCode,
    expected: Vec<Vec<&'static str>>,
}

#[test]
fn every_refusal_alternative_is_one_clean_line_that_a_shell_reads_back_faithfully() {
    let fresh_in_repo = vec!["respawn", ID, "--fresh", "--in", REPO];
    let cases = [
        Case {
            name: "missing transcript",
            cwd_exists: true,
            branch_in_repo: false,
            branch_recorded: true,
            code: RefusalCode::MissingTranscript,
            expected: vec![vec!["respawn", ID, "--fresh"]],
        },
        Case {
            name: "branch still exists",
            cwd_exists: false,
            branch_in_repo: true,
            branch_recorded: true,
            code: RefusalCode::MissingDirectory,
            expected: vec![
                vec!["-C", REPO, "worktree", "add", CWD, BRANCH],
                vec!["respawn", ID, "--recreate-worktree"],
                fresh_in_repo.clone(),
            ],
        },
        Case {
            name: "branch gone",
            cwd_exists: false,
            branch_in_repo: false,
            branch_recorded: true,
            code: RefusalCode::BranchGone,
            expected: vec![fresh_in_repo.clone()],
        },
        Case {
            name: "branch unrecorded",
            cwd_exists: false,
            branch_in_repo: false,
            branch_recorded: false,
            code: RefusalCode::MissingDirectory,
            expected: vec![fresh_in_repo],
        },
    ];
    for case in cases {
        let name = case.name;
        let fake = FakeEnv::new();
        fake.vars.set("HOME", "/home/u");
        fake.fs.add_dir(format!("{REPO}/.git/refs/heads"));
        if case.branch_in_repo {
            fake.fs
                .add_file(format!("{REPO}/.git/refs/heads/{BRANCH}"), "abc\n");
        }
        if case.cwd_exists {
            fake.fs.add_dir(CWD);
        }
        let mut s = Session::new(ID, "claude", "agent-1", CWD, AT);
        s.state = State::Interrupted;
        s.repo_root = Some(REPO.to_owned());
        s.branch = case.branch_recorded.then(|| BRANCH.to_owned());
        let r = plan_respawn(&s, &fake.env, &Options::default()).unwrap_err();
        assert_eq!(r.code, case.code, "{name}");
        assert_eq!(r.alternatives.len(), case.expected.len(), "{name}");
        for (alternative, want) in r.alternatives.iter().zip(&case.expected) {
            let line = &alternative.command;
            assert!(
                !line.chars().any(char::is_control),
                "{name}: control byte in {line:?}"
            );
            let program = line.split(' ').next().unwrap();
            assert_eq!(&bash_words(line, program), want, "{name}: {line:?}");
        }
    }
}
