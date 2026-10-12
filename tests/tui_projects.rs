#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! The grouped-by-project layout of the dashboard (T111, FR-019b).

use claude_monitor::model::State;
use claude_monitor::store::queries::{OverviewEntry, Section, SectionName};
use claude_monitor::tui::projects::{flat, grouped, Part, Scope};

fn entry(id: &str, project: Option<&str>, section: SectionName) -> OverviewEntry {
    OverviewEntry {
        id: id.to_owned(),
        agent_kind: "claude".to_owned(),
        agent_session_id: format!("agent-{id}"),
        repo: None,
        branch: None,
        cwd: "/work".to_owned(),
        state: State::Idle,
        running: false,
        last_activity_at: "2026-10-12T11:58:00Z".to_owned(),
        task_description: None,
        left_off: None,
        project: project.map(str::to_owned),
        flags: Vec::new(),
        section,
        error: None,
    }
}

fn sections(rows: &[(&str, Option<&str>, SectionName)]) -> Vec<Section> {
    [
        SectionName::Active,
        SectionName::Interrupted,
        SectionName::Finished,
        SectionName::Done,
        SectionName::Unseen,
        SectionName::Dismissed,
    ]
    .into_iter()
    .map(|name| Section {
        name,
        sessions: rows
            .iter()
            .filter(|(_, _, section)| *section == name)
            .map(|(id, project, section)| entry(id, *project, *section))
            .collect(),
    })
    .collect()
}

fn shown(sections: &[Section]) -> Vec<(&Section, Vec<&OverviewEntry>)> {
    sections
        .iter()
        .map(|section| (section, section.sessions.iter().collect()))
        .collect()
}

fn layout(parts: &[Part<'_>]) -> Vec<(String, Vec<String>)> {
    parts
        .iter()
        .map(|part| {
            (
                part.title(),
                part.entries.iter().map(|e| e.id.clone()).collect(),
            )
        })
        .collect()
}

fn ids(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn flat_keeps_the_sections_and_titles_them_plainly() {
    let data = sections(&[
        ("a", Some("P"), SectionName::Active),
        ("f", None, SectionName::Finished),
    ]);

    let parts = flat(shown(&data));

    assert!(parts.iter().all(|p| p.scope == Scope::Global));
    let titles: Vec<String> = parts.iter().map(Part::title).collect();
    assert_eq!(
        titles,
        [
            "Active",
            "Interrupted",
            "Finished",
            "Done",
            "Unseen",
            "Dismissed"
        ]
    );
    assert_eq!(parts[0].entries.len(), 1);
    assert_eq!(parts[0].total, 1);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn each_project_gets_its_own_sections_in_active_interrupted_finished_done_order() {
    let data = sections(&[
        ("a1", Some("Beta"), SectionName::Active),
        ("d1", Some("Beta"), SectionName::Done),
        ("f1", Some("Beta"), SectionName::Finished),
        ("i1", Some("Beta"), SectionName::Interrupted),
        ("a2", Some("Alpha"), SectionName::Active),
    ]);

    let parts = grouped(shown(&data));

    assert_eq!(
        layout(&parts),
        vec![
            ("Alpha · Active".to_owned(), ids(&["a2"])),
            ("Beta · Active".to_owned(), ids(&["a1"])),
            ("Beta · Interrupted".to_owned(), ids(&["i1"])),
            ("Beta · Finished".to_owned(), ids(&["f1"])),
            ("Beta · Done".to_owned(), ids(&["d1"])),
        ]
    );
    assert_eq!(parts[0].scope, Scope::Project("Alpha"));
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn a_projects_done_section_lists_only_its_own_done_sessions() {
    let data = sections(&[
        ("dp", Some("P"), SectionName::Done),
        ("dq", Some("Q"), SectionName::Done),
        ("dn", None, SectionName::Done),
    ]);

    let parts = grouped(shown(&data));

    assert_eq!(
        layout(&parts),
        vec![
            ("P · Done".to_owned(), ids(&["dp"])),
            ("Q · Done".to_owned(), ids(&["dq"])),
            ("No project · Done".to_owned(), ids(&["dn"])),
        ]
    );
    assert_eq!(parts[2].scope, Scope::NoProject);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn sessions_in_no_project_follow_the_projects_with_all_four_sections() {
    let data = sections(&[
        ("a", None, SectionName::Active),
        ("p", Some("P"), SectionName::Active),
        ("f", None, SectionName::Finished),
    ]);

    let parts = grouped(shown(&data));

    assert_eq!(
        layout(&parts),
        vec![
            ("P · Active".to_owned(), ids(&["p"])),
            ("No project · Active".to_owned(), ids(&["a"])),
            ("No project · Finished".to_owned(), ids(&["f"])),
        ]
    );
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn unseen_and_dismissed_stay_global_and_last_whatever_their_project() {
    let data = sections(&[
        ("u", Some("P"), SectionName::Unseen),
        ("x", Some("P"), SectionName::Dismissed),
        ("a", Some("P"), SectionName::Active),
    ]);

    let parts = grouped(shown(&data));

    assert_eq!(
        layout(&parts),
        vec![
            ("P · Active".to_owned(), ids(&["a"])),
            ("Unseen".to_owned(), ids(&["u"])),
            ("Dismissed".to_owned(), ids(&["x"])),
        ]
    );
    assert_eq!(parts[1].scope, Scope::Global);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn a_collapsed_unseen_section_keeps_its_full_count() {
    let data = sections(&[
        ("u1", None, SectionName::Unseen),
        ("u2", None, SectionName::Unseen),
    ]);
    let collapsed: Vec<(&Section, Vec<&OverviewEntry>)> =
        data.iter().map(|section| (section, Vec::new())).collect();

    let parts = grouped(collapsed);

    let unseen = parts
        .iter()
        .find(|p| p.name == SectionName::Unseen)
        .unwrap();
    assert!(unseen.entries.is_empty());
    assert_eq!(unseen.total, 2);
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn rows_the_filters_removed_leave_no_empty_project_part() {
    let data = sections(&[("a", Some("P"), SectionName::Active)]);
    let nothing_shown: Vec<(&Section, Vec<&OverviewEntry>)> =
        data.iter().map(|section| (section, Vec::new())).collect();

    let parts = grouped(nothing_shown);

    assert!(
        parts.iter().all(|p| p.scope == Scope::Global),
        "{:?}",
        layout(&parts)
    );
}

#[test]
#[ignore = "T111: implemented in the next commit"]
fn the_order_inside_a_section_is_kept() {
    let data = sections(&[
        ("a", Some("P"), SectionName::Active),
        ("b", Some("P"), SectionName::Active),
        ("c", Some("P"), SectionName::Active),
    ]);

    let parts = grouped(shown(&data));

    assert_eq!(parts[0].entries.len(), 3);
    let order: Vec<&str> = parts[0].entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(order, ["a", "b", "c"]);
}
