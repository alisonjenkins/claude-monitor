//! SC-011 for the terminal UI: every command tagged with a `tui_action` has a
//! real key in the dashboard, read from the table the key handler dispatches
//! through, so the two cannot drift apart.
#![cfg(feature = "tui")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{BTreeSet, HashMap};

use claude_monitor::cli::Command;
use claude_monitor::store::queries::{Overview, OverviewEntry, Section, SectionName};
use claude_monitor::tui::app::{update, App, Msg, Snapshot};
use claude_monitor::tui::bindings::{tui_bindings, LAUNCHED_BY_BINARY};
use claude_monitor::tui::view::view;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn tagged_actions() -> BTreeSet<&'static str> {
    Command::all()
        .iter()
        .filter_map(|info| info.tui_action)
        .collect()
}

#[test]
#[ignore = "I2: the new dashboard keys"]
fn every_tui_action_of_a_command_has_a_key_binding() {
    let mut missing = Vec::new();
    for info in Command::all() {
        let Some(action) = info.tui_action else {
            continue;
        };
        let bound = tui_bindings()
            .iter()
            .any(|binding| binding.commands.contains(&action));
        if !bound && !LAUNCHED_BY_BINARY.contains(&action) {
            missing.push(format!("{} ({action})", info.name));
        }
    }

    assert!(
        missing.is_empty(),
        "commands whose TUI action has no key: {missing:?}"
    );
}

#[test]
fn every_binding_names_a_real_tui_action_and_keys_are_unique() {
    let actions = tagged_actions();
    let mut keys = BTreeSet::new();
    for binding in tui_bindings() {
        assert!(keys.insert(binding.key), "{} is bound twice", binding.key);
        for command in binding.commands {
            assert!(
                actions.contains(command),
                "{} claims {command:?}, which no command tags",
                binding.key
            );
        }
    }
    for action in LAUNCHED_BY_BINARY {
        assert!(actions.contains(action), "{action:?} is not a tui_action");
    }
}

#[test]
#[ignore = "I2: the new dashboard keys"]
fn the_help_lists_every_key_that_carries_a_command() {
    let snapshot = Snapshot {
        overview: Overview {
            sections: vec![Section {
                name: SectionName::Active,
                sessions: Vec::<OverviewEntry>::new(),
            }],
        },
        panes: HashMap::new(),
        figures: HashMap::new(),
        now: "2026-10-12T12:00:00Z".to_owned(),
    };
    let app = update(App::new(), Msg::Refreshed(snapshot)).0;
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    terminal.draw(|frame| view(&app, frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let height = buffer.area.height;
    let mut help = String::new();
    for y in height.saturating_sub(4)..height {
        for x in 0..buffer.area.width {
            help.push_str(buffer[(x, y)].symbol());
        }
        help.push('\n');
    }
    let shown: BTreeSet<&str> = help.split_whitespace().collect();

    for binding in tui_bindings().iter().filter(|b| !b.commands.is_empty()) {
        assert!(
            shown.contains(binding.key.to_string().as_str()),
            "{} is not in the help: {help}",
            binding.key
        );
    }
}
