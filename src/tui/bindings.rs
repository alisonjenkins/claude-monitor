//! The dashboard's character keys as data. The key handler dispatches through
//! [`tui_bindings`], so the table the parity test reads is the one in use.

/// What a bound key does on the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Next,
    Previous,
    ToggleGrouped,
    Filter,
    CycleState,
    CycleAgent,
    ToggleDismissed,
    Done,
    Undone,
    Dismiss,
    Note,
    TogglePark,
    Group,
    Link,
    Summarize,
    Detail,
    Links,
    Restore,
    Hide,
    ToggleUnseen,
    Quit,
}

/// One character key.
#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub key: char,
    pub action: Action,
    /// The `tui_action` names in `cli::command` that this key carries out.
    pub commands: &'static [&'static str],
}

const fn bind(key: char, action: Action, commands: &'static [&'static str]) -> Binding {
    Binding {
        key,
        action,
        commands,
    }
}

const BINDINGS: &[Binding] = &[
    bind('j', Action::Next, &[]),
    bind('k', Action::Previous, &[]),
    bind('p', Action::ToggleGrouped, &[]),
    bind('/', Action::Filter, &[]),
    bind('f', Action::CycleState, &[]),
    bind('a', Action::CycleAgent, &[]),
    bind('S', Action::ToggleDismissed, &[]),
    bind('D', Action::Done, &["done"]),
    bind('U', Action::Undone, &["undone"]),
    bind('x', Action::Dismiss, &["dismiss", "undismiss"]),
    bind('n', Action::Note, &["note"]),
    bind('P', Action::TogglePark, &["park", "unpark"]),
    bind('g', Action::Group, &["group"]),
    bind('l', Action::Link, &["link"]),
    bind('s', Action::Summarize, &["summarize"]),
    bind('i', Action::Detail, &[]),
    bind('L', Action::Links, &[]),
    bind('R', Action::Restore, &["restore selection", "respawn"]),
    bind('d', Action::Hide, &[]),
    bind('u', Action::ToggleUnseen, &[]),
    bind('q', Action::Quit, &[]),
];

/// `tui_action` names the binary itself carries out: no key opens the
/// dashboard, running it does.
pub const LAUNCHED_BY_BINARY: &[&str] = &["open dashboard"];

pub fn tui_bindings() -> &'static [Binding] {
    BINDINGS
}

/// The action bound to `key`, if any.
pub fn action_for(key: char) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|binding| binding.key == key)
        .map(|binding| binding.action)
}
