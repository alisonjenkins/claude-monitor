//! Progress parsers: spec-kit tasks.md, Markdown checklists, todo payloads (T083).
//!
//! The "never panics" and invariant checks run a seeded generator rather than
//! a property-testing crate, so the dependency set stays unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use claude_monitor::model::StepStatus;
use claude_monitor::progress::{checklist, speckit, todo, Progress, SourceKind};

const SPECKIT: &str = include_str!("fixtures/progress/speckit_tasks.md");
const CHECKLIST: &str = include_str!("fixtures/progress/checklist.md");
const TODO: &str = include_str!("fixtures/progress/todo_payload.json");

fn ten_tasks_four_done() -> String {
    let mut s = String::from("## Phase 1: Only\n\n");
    for n in 1..=10 {
        let mark = if n <= 4 { "x" } else { " " };
        s.push_str(&format!("- [{mark}] T{n:03} task number {n}\n"));
    }
    s
}

#[test]
fn four_of_ten_with_current_item() {
    let p = speckit::parse(&ten_tasks_four_done(), Some("tasks.md")).unwrap();
    assert_eq!((p.done, p.total), (4, 10));
    assert_eq!(p.label(), "4 of 10");
    let cur = p.current.unwrap();
    assert_eq!(cur.external_id.as_deref(), Some("T005"));
    assert_eq!(cur.text, "task number 5");
}

#[test]
fn speckit_fixture_counts_phases_and_source() {
    let p = speckit::parse(SPECKIT, Some("specs/008/tasks.md")).unwrap();
    assert_eq!((p.done, p.total), (4, 9));
    assert_eq!(
        p.current.as_ref().unwrap().external_id.as_deref(),
        Some("T008")
    );
    let phases: Vec<_> = p
        .phases
        .iter()
        .map(|c| (c.name.as_str(), c.done, c.total))
        .collect();
    assert_eq!(
        phases,
        vec![
            ("Phase 1: Setup (shared)", 3, 3),
            ("Phase 2: Foundational (blocks all stories)", 1, 3),
            (
                "Phase 6: User Story 4 — where large projects stand (P2)",
                0,
                3
            ),
        ]
    );
    assert_eq!(p.source.kind, SourceKind::SpeckitTasks);
    assert_eq!(p.source.path.as_deref(), Some("specs/008/tasks.md"));
    assert_eq!(p.source_label(), "specs/008/tasks.md (spec-kit tasks)");
}

#[test]
fn speckit_strips_markers_and_keeps_ids() {
    let p = speckit::parse(SPECKIT, None).unwrap();
    let ids: Vec<_> = p
        .items
        .iter()
        .filter_map(|i| i.external_id.as_deref())
        .collect();
    assert_eq!(
        ids,
        ["T001", "T002", "T006a", "T007", "T008", "T009", "T083", "T084", "T086"]
    );
    let t83 = p
        .items
        .iter()
        .find(|i| i.external_id.as_deref() == Some("T083"))
        .unwrap();
    assert_eq!(t83.text, "Parser tests incl. property tests");
    assert_eq!(t83.status, StepStatus::Todo);
    let ordinals: Vec<u32> = p.items.iter().map(|i| i.ordinal).collect();
    assert_eq!(ordinals, (0..9).collect::<Vec<u32>>());
}

#[test]
fn speckit_ignores_checkboxes_without_task_ids() {
    assert!(speckit::parse("- [ ] just a bullet\n- [x] another\n", None).is_none());
}

#[test]
fn checklist_fixture() {
    let p = checklist::parse(CHECKLIST, Some("PENDING.md")).unwrap();
    assert_eq!((p.done, p.total), (2, 6));
    assert_eq!(
        p.current.as_ref().unwrap().text,
        "niks3 cache-push enablement"
    );
    assert_eq!(p.source.kind, SourceKind::MarkdownChecklist);
    assert_eq!(p.source_label(), "PENDING.md (Markdown checklist)");
    let phases: Vec<_> = p
        .phases
        .iter()
        .map(|c| (c.name.as_str(), c.done, c.total))
        .collect();
    assert_eq!(phases, vec![("Pending", 2, 5), ("Later", 0, 1)]);
}

#[test]
fn checklist_without_boxes_is_none() {
    assert!(checklist::parse("# Title\n\n- plain\n- [?] odd\n", None).is_none());
    assert!(checklist::parse("", None).is_none());
}

#[test]
fn fences_and_crlf_are_handled() {
    let text =
        "- [x] a\r\n```\r\n- [ ] hidden\r\n```\r\n- [ ] b\r\n~~~\r\n- [ ] hidden too\r\n~~~\r\n";
    let p = checklist::parse(text, None).unwrap();
    assert_eq!((p.done, p.total), (1, 2));
    assert_eq!(p.current.unwrap().text, "b");
}

#[test]
fn all_done_has_no_current() {
    let p = checklist::parse("- [x] a\n- [X] b\n", None).unwrap();
    assert_eq!((p.done, p.total), (2, 2));
    assert!(p.current.is_none());
}

#[test]
fn todo_payload_fixture() {
    let p = todo::parse(TODO).unwrap();
    assert_eq!((p.done, p.total), (1, 3));
    let cur = p.current.as_ref().unwrap();
    assert_eq!(cur.text, "Write the parser");
    assert_eq!(cur.status, StepStatus::Doing);
    assert_eq!(p.source.kind, SourceKind::AgentTodo);
    assert_eq!(p.source.path, None);
    assert_eq!(p.source_label(), "agent todo list");
}

#[test]
fn todo_tolerates_alternate_shapes() {
    let bare = r#"[{"text":"a","status":"completed"},{"title":"b","status":"pending"}]"#;
    let p = todo::parse(bare).unwrap();
    assert_eq!((p.done, p.total), (1, 2));
    assert_eq!(p.current.unwrap().text, "b");

    let wrapped = r#"{"tool_input":{"todos":[{"content":"x","status":"weird"}]}}"#;
    let p = todo::parse(wrapped).unwrap();
    assert_eq!((p.done, p.total), (0, 1));
}

#[test]
fn todo_unknown_shapes_are_none() {
    for input in [
        "",
        "null",
        "42",
        "\"s\"",
        "{}",
        "[]",
        "{\"todos\":[]}",
        "not json",
        "[1,2]",
        "[{\"status\":\"pending\"}]",
    ] {
        assert!(todo::parse(input).is_none(), "{input:?}");
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }
}

fn random_text(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "- [ ] ",
        "- [x] ",
        "- [X] ",
        "  - [ ] ",
        "* [ ] ",
        "[ ]",
        "[x]",
        "T001 ",
        "T12a ",
        "[P] ",
        "[US3] ",
        "## Phase 1: x\n",
        "## ",
        "# h\n",
        "### sub\n",
        "```\n",
        "~~~",
        "\n",
        "\r\n",
        "\t",
        "text ",
        "é",
        "—",
        "\u{0}",
        "-",
        "[",
        "]",
        " ",
        "1. [ ] ",
        "> - [ ] ",
    ];
    let n = rng.below(60);
    (0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

fn check_invariants(p: &Progress) {
    assert_eq!(usize::try_from(p.total).unwrap(), p.items.len());
    assert!(p.done <= p.total);
    let done = p
        .items
        .iter()
        .filter(|i| i.status == StepStatus::Done)
        .count();
    assert_eq!(usize::try_from(p.done).unwrap(), done);
    let first_open = p.items.iter().find(|i| i.status != StepStatus::Done);
    assert_eq!(p.current.as_ref(), first_open);
    let phase_total: u32 = p.phases.iter().map(|c| c.total).sum();
    assert!(phase_total <= p.total);
    assert!(p.phases.iter().all(|c| c.done <= c.total));
}

#[test]
fn random_markdown_never_panics_and_keeps_invariants() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..3000 {
        let text = random_text(&mut rng);
        for p in [
            speckit::parse(&text, None),
            checklist::parse(&text, Some("f.md")),
        ]
        .into_iter()
        .flatten()
        {
            check_invariants(&p);
        }
    }
}

#[test]
fn random_json_never_panics() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    const PIECES: &[&str] = &[
        "[",
        "]",
        "{",
        "}",
        ",",
        ":",
        "\"todos\"",
        "\"content\"",
        "\"status\"",
        "\"pending\"",
        "\"completed\"",
        "\"in_progress\"",
        "\"x\"",
        "1",
        "null",
        "true",
        " ",
    ];
    for _ in 0..3000 {
        let n = rng.below(40);
        let text: String = (0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect();
        if let Some(p) = todo::parse(&text) {
            check_invariants(&p);
        }
    }
}

/// One phase per task made the phase lookup quadratic: seconds for 60k
/// phases, milliseconds with a map.
#[test]
fn a_file_with_a_phase_per_task_parses_in_linear_time() {
    let mut text = String::new();
    for n in 0..60_000 {
        text.push_str(&format!("## Phase {n}: p{n}\n- [ ] T{n:05} task\n"));
    }
    let started = std::time::Instant::now();
    let progress = speckit::parse(&text, None).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(progress.phases.len(), 60_000);
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "parsing took {elapsed:?}"
    );
}
