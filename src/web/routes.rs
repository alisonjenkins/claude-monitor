//! The pages and actions of the web view, and the table that says which
//! library command each one offers (SC-011).

/// One library command and the web route that offers it. A `{id}` segment is
/// a session or project id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandRoute {
    /// The command's kebab-case name, as in `Command::all()`.
    pub command: &'static str,
    /// `GET` for a read, `POST` for a write (which needs the CSRF header).
    pub method: &'static str,
    pub path: &'static str,
}

/// Every command the web view offers. `tests/parity_web.rs` holds the commands
/// that are deliberately not here, each with its reason.
pub const COMMAND_ROUTES: &[CommandRoute] = &[];
