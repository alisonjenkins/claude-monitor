//! Every command in `contracts/cli.md` as data, so the CLI, the TUI, the web
//! view and the conformance tests enumerate the same list.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAction {
    Add,
    Rename,
    SetStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// No subcommand: the interactive overview.
    Dashboard,
    List {
        states: Vec<String>,
        agent: Option<String>,
        repo: Option<String>,
        project: Option<String>,
        text: Option<String>,
        section: Option<String>,
        include_dismissed: bool,
        expand_finished: bool,
        expand_unseen: bool,
    },
    Show {
        session: String,
    },
    Hook {
        agent: Option<String>,
        event: Option<String>,
    },
    Reconcile {
        pane: Option<String>,
        session: Option<String>,
    },
    Watch,
    Restore {
        all_interrupted: bool,
        include_finished: bool,
        fresh: bool,
        /// Sessions to leave out of the restore, by id or id prefix.
        skip: Vec<String>,
        yes: bool,
    },
    Respawn {
        sessions: Vec<String>,
        fresh: bool,
        in_dir: Option<PathBuf>,
        recreate_worktree: bool,
    },
    BootCheck {
        banner: bool,
    },
    /// `project add|rename|set-status <name> [value]`; `value` is the new
    /// name for `rename` and the status for `set-status`.
    Project {
        action: ProjectAction,
        name: String,
        value: Option<String>,
    },
    Group {
        project: String,
        sessions: Vec<String>,
    },
    Note {
        target: String,
        text: String,
    },
    Park {
        target: String,
    },
    Unpark {
        target: String,
    },
    Dismiss {
        target: String,
    },
    Undismiss {
        target: String,
    },
    Done {
        sessions: Vec<String>,
    },
    Undone {
        sessions: Vec<String>,
    },
    ProgressBind {
        owner: String,
        file: Option<PathBuf>,
        source: Option<String>,
    },
    Summarize {
        target: String,
    },
    Serve {
        port: Option<u16>,
    },
    Link {
        owner: String,
        cortex: String,
        kind: String,
    },
    ExportCortexiphan {
        project: Option<String>,
        out: Option<PathBuf>,
    },
    LinksCheck,
    Migrate,
    Delete {
        session: String,
        yes: bool,
    },
}

/// Metadata for one command.
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    /// Kebab-case command name, for example `boot-check`.
    pub name: &'static str,
    /// The TUI action that runs this command, when the TUI offers one. A
    /// command with a TUI action is "interactive".
    pub tui_action: Option<&'static str>,
    /// The task that implements the command; it removes the command's
    /// `#[ignore]` in `tests/cli_conformance.rs`.
    pub implemented_by: &'static str,
    /// A valid instance of the command, for tests that run every command.
    pub example: fn() -> Command,
}

impl CommandInfo {
    pub fn is_interactive(&self) -> bool {
        self.tui_action.is_some()
    }
}

fn s(v: &str) -> String {
    v.to_owned()
}

const ALL: &[CommandInfo] = &[
    CommandInfo {
        name: "dashboard",
        tui_action: Some("open dashboard"),
        implemented_by: "existing",
        example: || Command::Dashboard,
    },
    CommandInfo {
        name: "list",
        tui_action: None,
        implemented_by: "T032",
        example: || Command::List {
            states: vec![],
            agent: None,
            repo: None,
            project: None,
            text: None,
            section: None,
            include_dismissed: false,
            expand_finished: false,
            expand_unseen: false,
        },
    },
    CommandInfo {
        name: "show",
        tui_action: None,
        implemented_by: "T032",
        example: || Command::Show { session: s("s1") },
    },
    CommandInfo {
        name: "hook",
        tui_action: None,
        implemented_by: "existing",
        example: || Command::Hook {
            agent: None,
            event: None,
        },
    },
    CommandInfo {
        name: "reconcile",
        tui_action: None,
        implemented_by: "T052",
        example: || Command::Reconcile {
            pane: None,
            session: None,
        },
    },
    CommandInfo {
        name: "watch",
        tui_action: None,
        implemented_by: "T060",
        example: || Command::Watch,
    },
    CommandInfo {
        name: "restore",
        tui_action: Some("restore selection"),
        implemented_by: "T073",
        example: || Command::Restore {
            all_interrupted: true,
            include_finished: false,
            fresh: false,
            skip: vec![],
            yes: true,
        },
    },
    CommandInfo {
        name: "respawn",
        tui_action: Some("respawn"),
        implemented_by: "T073",
        example: || Command::Respawn {
            sessions: vec![s("s1")],
            fresh: false,
            in_dir: None,
            recreate_worktree: false,
        },
    },
    CommandInfo {
        name: "boot-check",
        tui_action: None,
        implemented_by: "T075",
        example: || Command::BootCheck { banner: false },
    },
    CommandInfo {
        name: "project",
        tui_action: None,
        implemented_by: "T108",
        example: || Command::Project {
            action: ProjectAction::Add,
            name: s("p1"),
            value: None,
        },
    },
    CommandInfo {
        name: "group",
        tui_action: Some("group"),
        implemented_by: "T108",
        example: || Command::Group {
            project: s("p1"),
            sessions: vec![s("s1")],
        },
    },
    CommandInfo {
        name: "note",
        tui_action: Some("note"),
        implemented_by: "T109",
        example: || Command::Note {
            target: s("s1"),
            text: s("a note"),
        },
    },
    CommandInfo {
        name: "park",
        tui_action: Some("park"),
        implemented_by: "T109",
        example: || Command::Park { target: s("s1") },
    },
    CommandInfo {
        name: "unpark",
        tui_action: Some("unpark"),
        implemented_by: "T109",
        example: || Command::Unpark { target: s("s1") },
    },
    CommandInfo {
        name: "dismiss",
        tui_action: Some("dismiss"),
        implemented_by: "T109",
        example: || Command::Dismiss { target: s("s1") },
    },
    CommandInfo {
        name: "undismiss",
        tui_action: Some("undismiss"),
        implemented_by: "T109",
        example: || Command::Undismiss { target: s("s1") },
    },
    CommandInfo {
        name: "done",
        tui_action: Some("done"),
        implemented_by: "T110",
        example: || Command::Done {
            sessions: vec![s("s1")],
        },
    },
    CommandInfo {
        name: "undone",
        tui_action: Some("undone"),
        implemented_by: "T110",
        example: || Command::Undone {
            sessions: vec![s("s1")],
        },
    },
    CommandInfo {
        name: "progress-bind",
        tui_action: Some("choose progress source"),
        implemented_by: "T088",
        example: || Command::ProgressBind {
            owner: s("s1"),
            file: None,
            source: None,
        },
    },
    CommandInfo {
        name: "summarize",
        tui_action: Some("summarize"),
        implemented_by: "T092",
        example: || Command::Summarize { target: s("s1") },
    },
    CommandInfo {
        name: "serve",
        tui_action: None,
        implemented_by: "T117",
        example: || Command::Serve { port: None },
    },
    CommandInfo {
        name: "link",
        tui_action: Some("link"),
        implemented_by: "T100",
        example: || Command::Link {
            owner: s("s1"),
            cortex: s("node-1"),
            kind: s("task"),
        },
    },
    CommandInfo {
        name: "export-cortexiphan",
        tui_action: None,
        implemented_by: "T098",
        example: || Command::ExportCortexiphan {
            project: None,
            out: None,
        },
    },
    CommandInfo {
        name: "links-check",
        tui_action: None,
        implemented_by: "T101",
        example: || Command::LinksCheck,
    },
    CommandInfo {
        name: "migrate",
        tui_action: None,
        implemented_by: "T020",
        example: || Command::Migrate,
    },
    CommandInfo {
        name: "delete",
        tui_action: None,
        implemented_by: "unscheduled (no task in tasks.md)",
        example: || Command::Delete {
            session: s("s1"),
            yes: true,
        },
    },
];

impl Command {
    /// Kebab-case name, matching [`CommandInfo::name`].
    pub fn name(&self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::List { .. } => "list",
            Self::Show { .. } => "show",
            Self::Hook { .. } => "hook",
            Self::Reconcile { .. } => "reconcile",
            Self::Watch => "watch",
            Self::Restore { .. } => "restore",
            Self::Respawn { .. } => "respawn",
            Self::BootCheck { .. } => "boot-check",
            Self::Project { .. } => "project",
            Self::Group { .. } => "group",
            Self::Note { .. } => "note",
            Self::Park { .. } => "park",
            Self::Unpark { .. } => "unpark",
            Self::Dismiss { .. } => "dismiss",
            Self::Undismiss { .. } => "undismiss",
            Self::Done { .. } => "done",
            Self::Undone { .. } => "undone",
            Self::ProgressBind { .. } => "progress-bind",
            Self::Summarize { .. } => "summarize",
            Self::Serve { .. } => "serve",
            Self::Link { .. } => "link",
            Self::ExportCortexiphan { .. } => "export-cortexiphan",
            Self::LinksCheck => "links-check",
            Self::Migrate => "migrate",
            Self::Delete { .. } => "delete",
        }
    }

    /// Every command, in the order of `contracts/cli.md`.
    pub fn all() -> &'static [CommandInfo] {
        ALL
    }

    /// Metadata for this command.
    pub fn info(&self) -> Option<&'static CommandInfo> {
        ALL.iter().find(|info| info.name == self.name())
    }

    /// The command name behind a TUI action, for example `"respawn"`.
    pub fn for_tui_action(action: &str) -> Option<&'static str> {
        ALL.iter()
            .find(|info| info.tui_action == Some(action))
            .map(|info| info.name)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_entry_builds_a_command_with_its_own_name() {
        for info in Command::all() {
            assert_eq!((info.example)().name(), info.name);
            assert_eq!((info.example)().info().unwrap().name, info.name);
        }
    }

    #[test]
    fn names_are_unique_kebab_case_and_cover_the_contract() {
        let names: Vec<_> = Command::all().iter().map(|i| i.name).collect();
        let unique: BTreeSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        for name in &names {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name}"
            );
        }
        // 25 contract commands plus the no-subcommand dashboard.
        assert_eq!(names.len(), 26);
    }

    #[test]
    fn tui_actions_resolve_to_commands() {
        assert_eq!(Command::for_tui_action("respawn"), Some("respawn"));
        assert_eq!(
            Command::for_tui_action("choose progress source"),
            Some("progress-bind")
        );
        assert_eq!(Command::for_tui_action("nonsense"), None);
        assert!(Command::Dashboard.info().unwrap().is_interactive());
        assert!(!Command::Watch.info().unwrap().is_interactive());
    }
}
