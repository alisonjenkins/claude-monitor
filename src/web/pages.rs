//! The HTML of the web view (T116, `docs/web-design.md`). Pure functions from
//! the JSON the CLI prints to markup: nothing here reads the store, and every
//! string from a session is escaped by maud.

use std::collections::HashMap;

use maud::{html, Markup, DOCTYPE};
use serde_json::Value;

use crate::cli::age;
use crate::cli::show::placement_text;

/// What every page needs besides its content.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// Echoed into `<meta name="cc-csrf">` for the page's own requests.
    pub csrf: &'a str,
    pub now: &'a str,
}

/// A stored plan figure for an overview row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowProgress {
    pub done: u32,
    pub total: u32,
    pub source: String,
}

/// The filter form's current values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub state: String,
    pub agent: String,
    pub repo: String,
    pub project: String,
    pub text: String,
    pub include_dismissed: bool,
}

const STATES: [&str; 6] = [
    "working",
    "needs_permission",
    "idle",
    "finished",
    "interrupted",
    "unknown",
];

const SHORT_ID: usize = 8;
const TASK_FALLBACK: &str = "(no task recorded)";

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn list_of<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

const NO_PLACEMENT: &str = "no recorded tmux window";

/// `show`'s tmux line for a session JSON object; `None` when no tmux session
/// was ever recorded.
fn placement_of(detail: &Value) -> Option<String> {
    Some(placement_text(
        str_of(detail, "tmux_session")?,
        str_of(detail, "tmux_window_name"),
        str_of(detail, "tmux_window_index"),
        str_of(detail, "tmux_pane_index"),
        str_of(detail, "tmux_window_panes"),
    ))
}

/// A path segment safe to put in a URL.
pub(super) fn seg(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn state_words(state: &str) -> String {
    state.replace('_', " ")
}

fn short(id: &str) -> String {
    id.chars().take(SHORT_ID).collect()
}

fn is_waiting(entry: &Value) -> bool {
    let section = str_of(entry, "section").unwrap_or_default();
    let state = str_of(entry, "state").unwrap_or_default();
    section != "dismissed" && (state == "needs_permission" || state == "interrupted")
}

fn layout(ctx: &Ctx<'_>, title: &str, waiting: usize, region: &str, body: Markup) -> Markup {
    let full_title = if waiting > 0 {
        format!("({waiting}) {title}")
    } else {
        title.to_owned()
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                meta name="cc-csrf" content=(ctx.csrf);
                title { (full_title) }
                link rel="stylesheet" href="/assets/app.css";
                script src="/assets/htmx.min.js" defer {}
                script src="/assets/app.js" defer {}
            }
            body {
                a.skip href=(format!("#{region}")) { "Skip to sessions" }
                header.top {
                    a.brand href="/" { "claude-monitor" }
                    nav aria-label="Pages" {
                        a href="/" { "Overview" }
                        a href="/restore" { "Restore" }
                        a href="/links" { "Links" }
                    }
                }
                div #flash role="status" aria-live="polite" {}
                main { (body) }
            }
        }
    }
}

/// A region that re-reads `url` whenever the change feed says something
/// changed, and swaps only itself.
fn live(id: &str, url: &str, waiting: usize, inner: Markup) -> Markup {
    html! {
        div id=(id) data-waiting=(waiting)
            hx-get=(url)
            hx-trigger="cc-changed from:body delay:150ms"
            hx-select=(format!("#{id}"))
            hx-swap="outerHTML" {
            (inner)
        }
    }
}

fn post_button(url: &str, label: &str, primary: bool) -> Markup {
    html! {
        button.btn.primary[primary] type="button" hx-post=(url) hx-swap="none" { (label) }
    }
}

/// Respawn starts a process, so the button goes to the preview first and the
/// start waits for the confirm there.
fn respawn_link(id: &str, primary: bool) -> Markup {
    html! {
        a.btn.primary[primary] href=(format!("/restore?session={}", seg(id))) { "Respawn" }
    }
}

fn row_actions(entry: &Value) -> Markup {
    let id = str_of(entry, "id").unwrap_or_default();
    let state = str_of(entry, "state").unwrap_or_default();
    let section = str_of(entry, "section").unwrap_or_default();
    let base = format!("/sessions/{}", seg(id));
    html! {
        @if state == "interrupted" {
            (respawn_link(id, true))
        } @else if section == "finished" {
            (post_button(&format!("{base}/done"), "Mark done", false))
        } @else if section == "done" {
            (post_button(&format!("{base}/undone"), "Unmark done", false))
        } @else if section == "dismissed" {
            (post_button(&format!("{base}/undismiss"), "Undismiss", false))
        }
    }
}

fn progress_figure(progress: &RowProgress) -> Markup {
    let percent = if progress.total == 0 {
        0
    } else {
        progress
            .done
            .saturating_mul(100)
            .checked_div(progress.total)
            .unwrap_or(0)
    };
    html! {
        p.meta {
            span.figure.mono { (progress.done) " of " (progress.total) }
            span.bar aria-hidden="true" { i style=(format!("width:{percent}%")) {} }
            span.src.mono { (progress.source) }
        }
    }
}

/// How a row is dressed where it appears.
#[derive(Clone, Copy)]
struct RowStyle<'a> {
    /// In the waiting band: the row id differs so the same session can also
    /// sit in its project's node.
    band: bool,
    /// Name the project in the row (the band, Unseen, Dismissed).
    show_project: bool,
    /// The projects a Move panel offers; `None` for no Move control.
    movable: Option<&'a [Value]>,
}

const BAND: RowStyle<'static> = RowStyle {
    band: true,
    show_project: true,
    movable: None,
};

/// The value of the "New project…" option; no project can be named "".
const NEW_PROJECT: &str = "";

/// The inline panel behind a row's Move button: no modal, shown by `app.js`.
fn move_panel(id: &str, current: Option<&str>, projects: &[Value]) -> Markup {
    let panel = format!("mv-{id}");
    html! {
        form.move id=(panel.clone()) hidden hx-post=(format!("/sessions/{}/group", seg(id)))
            hx-swap="none" data-move-form {
            div.field {
                label for=(format!("{panel}-to")) { "Move to" }
                select id=(format!("{panel}-to")) name="project" data-move-to {
                    @for project in projects {
                        @let name = str_of(project, "name").unwrap_or_default();
                        @let here = current == Some(name);
                        option value=(name) selected[here] disabled[here] {
                            (name) @if here { " (current)" }
                        }
                    }
                    option value="-" selected[current.is_none()] disabled[current.is_none()] {
                        "No project (remove)"
                    }
                    option value=(NEW_PROJECT) { "New project…" }
                }
            }
            div.field data-move-new-wrap hidden {
                label for=(format!("{panel}-new")) { "New project name" }
                input type="text" id=(format!("{panel}-new")) data-move-new autocomplete="off";
            }
            div.inline {
                button.btn.solid type="submit" disabled data-move-go { "Move" }
                button.btn.quiet type="button" data-move-cancel { "Cancel" }
            }
        }
    }
}

fn row(
    ctx: &Ctx<'_>,
    entry: &Value,
    progress: &HashMap<String, RowProgress>,
    style: RowStyle<'_>,
) -> Markup {
    let id = str_of(entry, "id").unwrap_or_default();
    let state = str_of(entry, "state").unwrap_or_default();
    let task = str_of(entry, "error")
        .or_else(|| str_of(entry, "task_description"))
        .unwrap_or(TASK_FALLBACK);
    let place = match (str_of(entry, "repo"), str_of(entry, "branch")) {
        (Some(repo), Some(branch)) => format!("{repo}/{branch}"),
        (Some(repo), None) => repo.to_owned(),
        _ => str_of(entry, "cwd").unwrap_or_default().to_owned(),
    };
    let ago = str_of(entry, "last_activity_at")
        .map(|then| age(ctx.now, then))
        .unwrap_or_else(|| "?".to_owned());
    let flags: Vec<&str> = list_of(entry, "flags")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    html! {
        li.row id=(format!("{}-{id}", if style.band { "w" } else { "s" })) data-state=(state) {
            span.state { i.dot {} (state_words(state)) }
            div.what {
                a.task href=(format!("/sessions/{}", seg(id))) { (task) }
                @if let Some(left) = str_of(entry, "left_off") {
                    p { (left) }
                }
                p.meta.mono {
                    (place) " · " (short(id))
                    @if let (true, Some(project)) = (style.show_project, str_of(entry, "project")) {
                        " · "
                        a.project href=(format!("/projects/{}", seg(project))) { (project) }
                    }
                    @for flag in &flags {
                        span.flag { (flag) }
                    }
                    @if let Some(hint) = str_of(entry, "tmux_hint") {
                        " · " span.muted { "tmux " (hint) }
                    }
                }
                @if let Some(figure) = progress.get(id) {
                    (progress_figure(figure))
                }
            }
            span.age.mono title=(str_of(entry, "last_activity_at").unwrap_or_default()) { (ago) }
            div.actions {
                (row_actions(entry))
                @if style.movable.is_some() {
                    button.btn.quiet type="button" id=(format!("mvb-{id}"))
                        data-move-toggle aria-expanded="false"
                        aria-controls=(format!("mv-{id}"))
                        aria-label=(format!("Move session {}", short(id))) { "Move" }
                }
            }
            @if let Some(projects) = style.movable {
                (move_panel(id, str_of(entry, "project"), projects))
            }
        }
    }
}

fn rows(
    ctx: &Ctx<'_>,
    entries: &[&Value],
    progress: &HashMap<String, RowProgress>,
    style: RowStyle<'_>,
) -> Markup {
    html! {
        ul.rows {
            @for entry in entries { (row(ctx, entry, progress, style)) }
        }
    }
}

fn in_section<'a>(entries: &'a [Value], name: &str) -> Vec<&'a Value> {
    entries
        .iter()
        .filter(|entry| str_of(entry, "section") == Some(name))
        .collect()
}

/// One group of a node's sessions (Active, Interrupted, Finished).
fn node_section(
    ctx: &Ctx<'_>,
    node_id: &str,
    name: &str,
    heading: &str,
    entries: &[&Value],
    progress: &HashMap<String, RowProgress>,
    style: RowStyle<'_>,
) -> Markup {
    html! {
        @if !entries.is_empty() {
            section id=(format!("{node_id}-section-{name}")) {
                h3 { (heading) span.count { "(" (entries.len()) ")" } }
                (rows(ctx, entries, progress, style))
            }
        }
    }
}

/// A node's `Done` group: nested and closed, because done work is out of the
/// way by definition; finished never is.
fn node_done(
    ctx: &Ctx<'_>,
    node_id: &str,
    entries: &[&Value],
    progress: &HashMap<String, RowProgress>,
    style: RowStyle<'_>,
) -> Markup {
    html! {
        @if !entries.is_empty() {
            section id=(format!("{node_id}-section-done")) {
                details id=(format!("{node_id}-done")) {
                    summary { "Done" span.count { "(" (entries.len()) ")" } }
                    (rows(ctx, entries, progress, style))
                }
            }
        }
    }
}

/// Rename, status, note and park for a project, folded away until wanted.
fn project_menu(node: &Value, id: &str) -> Markup {
    let base = format!("/projects/{}", seg(id));
    let name = str_of(node, "name").unwrap_or_default();
    let status = str_of(node, "status").unwrap_or_default();
    let parked = status == "parked";
    html! {
        details.menu id=(format!("m-{id}")) {
            summary { "Manage project" }
            div.menu-body {
                form.inline hx-post=(base.clone()) hx-swap="none" {
                    label for=(format!("m-{id}-name")) { "Rename" }
                    input type="text" id=(format!("m-{id}-name")) name="name" value=(name) required;
                    button.btn type="submit" { "Rename" }
                }
                form.inline hx-post=(base.clone()) hx-swap="none" {
                    label for=(format!("m-{id}-status")) { "Status" }
                    select id=(format!("m-{id}-status")) name="status" {
                        @for option in ["active", "parked", "done", "archived"] {
                            option value=(option) selected[status == option] { (option) }
                        }
                    }
                    button.btn type="submit" { "Set status" }
                }
                form.inline hx-post=(format!("{base}/note")) hx-swap="none" data-reset="" {
                    label for=(format!("m-{id}-note")) { "Note" }
                    input type="text" id=(format!("m-{id}-note")) name="text" required;
                    button.btn type="submit" { "Add note" }
                }
                div.inline {
                    @if parked {
                        (post_button(&format!("{base}/unpark"), "Unpark", false))
                    } @else {
                        (post_button(&format!("{base}/park"), "Park", false))
                    }
                    a.btn.quiet href=(base) { "Open project page" }
                }
            }
        }
    }
}

/// A project (or the sessions with none) and its sessions: Active,
/// Interrupted, Finished and a nested Done. Unseen and dismissed sessions are
/// gathered in their own nodes below, not here.
fn project_node(
    ctx: &Ctx<'_>,
    node: &Value,
    projects: &[Value],
    progress: &HashMap<String, RowProgress>,
    tools: bool,
) -> Markup {
    let project_id = str_of(node, "id");
    let node_id = format!("n-{}", project_id.unwrap_or("none"));
    let sessions = list_of(node, "sessions");
    let style = RowStyle {
        band: false,
        show_project: false,
        movable: Some(projects),
    };
    let active: Vec<&Value> = in_section(sessions, "active");
    let interrupted = in_section(sessions, "interrupted");
    let finished = in_section(sessions, "finished");
    let done = in_section(sessions, "done");
    let empty = [&active, &interrupted, &finished, &done]
        .iter()
        .all(|group| group.is_empty());
    let name = str_of(node, "name").unwrap_or("No project");
    let figure = node.get("progress").filter(|p| p.is_object());
    html! {
        details class=(if empty { "node empty" } else { "node" }) id=(node_id.clone()) data-node open {
            summary aria-expanded="true" {
                h2.node-name { (name) }
                @if let Some(status) = str_of(node, "status") {
                    span.badge { (status) }
                }
                span.node-summary.mono { (str_of(node, "summary").unwrap_or("no sessions")) }
                @if let Some(figure) = figure {
                    span.node-progress.mono {
                        (figure.get("done").and_then(Value::as_u64).unwrap_or(0))
                        " of "
                        (figure.get("total").and_then(Value::as_u64).unwrap_or(0))
                        " " span.src { (str_of(figure, "source").unwrap_or_default()) }
                    }
                }
            }
            div.node-body {
                @if let (true, Some(id)) = (tools, project_id) {
                    (project_menu(node, id))
                }
                @if empty {
                    p.muted { "No sessions here yet. Use Move on any session to bring one in." }
                }
                (node_section(ctx, &node_id, "active", "Active", &active, progress, style))
                (node_section(ctx, &node_id, "interrupted", "Interrupted", &interrupted, progress, style))
                (node_section(ctx, &node_id, "finished", "Finished", &finished, progress, style))
                (node_done(ctx, &node_id, &done, progress, style))
            }
        }
    }
}

/// The sessions region shared by the overview and a project page: the
/// waiting band first, then the tree of project nodes, then the nodes for
/// unseen and dismissed sessions. `only` limits the tree to one project.
fn sessions_region(
    ctx: &Ctx<'_>,
    tree: &Value,
    entries: &[Value],
    progress: &HashMap<String, RowProgress>,
    url: &str,
    only: Option<&str>,
) -> Markup {
    let waiting: Vec<&Value> = entries.iter().filter(|e| is_waiting(e)).collect();
    let interrupted = waiting
        .iter()
        .filter(|e| str_of(e, "state") == Some("interrupted"))
        .count();
    let projects = list_of(tree, "projects");
    let loose = tree.get("no_project").unwrap_or(&Value::Null);
    let unseen = in_section(entries, "unseen");
    let dismissed = in_section(entries, "dismissed");
    let shown = |node: &&Value| only.is_none_or(|id| str_of(node, "id") == Some(id));
    let listed: Vec<&Value> = projects.iter().filter(shown).collect();
    let show_loose = only.is_none()
        && ["active", "interrupted", "finished", "done"]
            .iter()
            .any(|name| !in_section(list_of(loose, "sessions"), name).is_empty());
    let outside = RowStyle {
        band: false,
        show_project: true,
        movable: Some(projects),
    };
    let inner = html! {
        @if !waiting.is_empty() {
            section #waiting aria-labelledby="waiting-h" {
                h2 #waiting-h { "Waiting on you" span.count { "(" (waiting.len()) ")" } }
                @if interrupted > 0 {
                    p.lede {
                        (interrupted) " interrupted. "
                        a href="/restore" { "Review and restore them together" }
                    }
                }
                (rows(ctx, &waiting, progress, BAND))
            }
        }
        div.tree {
            @for node in &listed {
                (project_node(ctx, node, projects, progress, only.is_none()))
            }
            @if show_loose {
                (project_node(ctx, loose, projects, progress, false))
            }
        }
        @if only.is_none() {
            details.menu id="m-new-project" {
                summary { "New empty project" }
                div.menu-body {
                    form.inline hx-post="/projects" hx-swap="none" data-reset="" {
                        label for="m-new-project-name" { "Project name" }
                        input type="text" id="m-new-project-name" name="name" required;
                        button.btn type="submit" { "Create" }
                    }
                }
            }
        }
        @if !unseen.is_empty() {
            details id="section-unseen" class="section" {
                summary { "Unseen sessions" span.count { "(" (unseen.len()) ")" } }
                (rows(ctx, &unseen, progress, outside))
            }
        }
        @if !dismissed.is_empty() {
            section id="section-dismissed" {
                h2 { "Dismissed" span.count { "(" (dismissed.len()) ")" } }
                (rows(ctx, &dismissed, progress, outside))
            }
        }
        @if entries.is_empty() && listed.is_empty() {
            p.muted { "No sessions match." }
        }
    };
    live("sessions", url, waiting.len(), inner)
}

fn waiting_count(entries: &[Value]) -> usize {
    entries.iter().filter(|e| is_waiting(e)).count()
}

fn filter_form(filters: &Filters) -> Markup {
    html! {
        form method="get" action="/sessions" class="filters" {
            label { "State"
                select name="state" {
                    option value="" selected[filters.state.is_empty()] { "any" }
                    @for state in STATES {
                        option value=(state) selected[filters.state == state] { (state_words(state)) }
                    }
                }
            }
            label { "Agent" input type="text" name="agent" value=(filters.agent); }
            label { "Repo" input type="text" name="repo" value=(filters.repo); }
            label { "Project" input type="text" name="project" value=(filters.project); }
            label { "Text" input type="text" name="text" value=(filters.text); }
            label.check {
                input type="checkbox" name="include_dismissed" value="true" checked[filters.include_dismissed];
                "Include dismissed"
            }
            button.btn type="submit" { "Filter" }
        }
    }
}

pub fn overview(
    ctx: &Ctx<'_>,
    tree: &Value,
    entries: &[Value],
    progress: &HashMap<String, RowProgress>,
    filters: &Filters,
    url: &str,
) -> String {
    let body = html! {
        h1 { "Sessions" }
        (filter_form(filters))
        (sessions_region(ctx, tree, entries, progress, url, None))
    };
    layout(
        ctx,
        "claude-monitor",
        waiting_count(entries),
        "sessions",
        body,
    )
    .into_string()
}

pub fn project(
    ctx: &Ctx<'_>,
    project: &Value,
    notes: &[Value],
    tree: &Value,
    entries: &[Value],
    progress: &HashMap<String, RowProgress>,
    url: &str,
) -> String {
    let id = str_of(project, "id").unwrap_or_default();
    let name = str_of(project, "name").unwrap_or_default();
    let status = str_of(project, "status").unwrap_or_default();
    let base = format!("/projects/{}", seg(id));
    let body = html! {
        h1 { (name) }
        p.muted { "Status " strong { (status) } }
        form hx-post=(base.clone()) hx-swap="none" class="inline" {
            label { "Status"
                select name="status" {
                    @for option in ["active", "parked", "done", "archived"] {
                        option value=(option) selected[status == option] { (option) }
                    }
                }
            }
            button.btn type="submit" { "Set status" }
        }
        form hx-post=(base.clone()) hx-swap="none" class="inline" {
            label { "Rename" input type="text" name="name" value=(name); }
            button.btn type="submit" { "Rename" }
        }
        (notes_panel(&format!("{base}/note"), notes))
        (sessions_region(ctx, tree, entries, progress, url, Some(id)))
    };
    layout(
        ctx,
        &format!("{name} · claude-monitor"),
        waiting_count(entries),
        "sessions",
        body,
    )
    .into_string()
}

fn notes_panel(post_url: &str, notes: &[Value]) -> Markup {
    html! {
        section.panel {
            h2 { "Notes" }
            @for note in notes {
                p.verbatim {
                    (str_of(note, "body").unwrap_or_default())
                    " " span.muted.mono { (str_of(note, "created_at").unwrap_or_default()) }
                }
            }
            form hx-post=(post_url) hx-swap="none" class="stack" data-reset="" {
                label { "Add a note" textarea name="text" required {} }
                div { button.btn type="submit" { "Add note" } }
            }
        }
    }
}

fn lineage_links(label: &str, ids: &[&str]) -> Markup {
    html! {
        @if !ids.is_empty() {
            dt { (label) }
            dd {
                @for id in ids {
                    a.mono href=(format!("/sessions/{}", seg(id))) { (id) } " "
                }
            }
        }
    }
}

fn progress_panel(session_id: &str, detail: &Value) -> Markup {
    let status = str_of(detail, "progress_status").unwrap_or("none");
    let post = format!("/sessions/{}/progress-source", seg(session_id));
    html! {
        section.panel {
            h2 { "Progress" }
            @match status {
                "bound" => {
                    @if let Some(progress) = detail.get("progress") {
                        p {
                            span.figure.mono {
                                (progress.get("done").and_then(Value::as_u64).unwrap_or(0))
                                " of "
                                (progress.get("total").and_then(Value::as_u64).unwrap_or(0))
                            }
                            span.src.mono { (str_of(progress, "source_label").unwrap_or_default()) }
                        }
                        @if progress.get("stale").and_then(Value::as_bool) == Some(true) {
                            p.warn { "The source can no longer be read, so this figure may be out of date." }
                        }
                        @if let Some(current) = str_of(progress, "current") {
                            p { "Current: " (current) }
                        }
                        @for phase in list_of(progress, "phases") {
                            p.mono.muted {
                                (str_of(phase, "name").unwrap_or_default()) " "
                                (phase.get("done").and_then(Value::as_u64).unwrap_or(0)) "/"
                                (phase.get("total").and_then(Value::as_u64).unwrap_or(0))
                            }
                        }
                        (post_button_with(&post, "Show no progress", r#"{"source":"none"}"#))
                    }
                }
                "pick_one" => {
                    p.warn { "Several plans could be this session's. Choose one; no figure is guessed." }
                    @for candidate in list_of(detail, "candidates") {
                        @let path = str_of(candidate, "path").unwrap_or_default();
                        div.inline {
                            span.mono { (path) }
                            span.muted.mono {
                                (candidate.get("done").and_then(Value::as_u64).unwrap_or(0))
                                " of "
                                (candidate.get("total").and_then(Value::as_u64).unwrap_or(0))
                            }
                            (post_button_with(
                                &post,
                                "Use this plan",
                                &serde_json::json!({ "path": path }).to_string(),
                            ))
                        }
                    }
                }
                _ => {
                    p.muted { "No plan found for this session." }
                }
            }
        }
    }
}

/// Asks `/…/dismiss` for the confirmation panel and shows it in the slot above
/// the page; nothing changes until the panel's own Confirm is pressed.
fn confirm_button(url: &str, label: &str) -> Markup {
    html! {
        button.btn.quiet type="button" hx-get=(url) hx-target="#confirm-slot" hx-swap="innerHTML"
            hx-select="unset" {
            (label)
        }
    }
}

/// What the confirmation panel says and where its buttons go.
pub struct Confirm<'a> {
    /// "Dismiss session s1"; the panel asks it as a question.
    pub action: &'a str,
    pub consequence: &'a str,
    /// The POST that performs the change once `confirm=1` is sent.
    pub post_url: &'a str,
    /// Form fields the POST needs besides `confirm`.
    pub hidden: &'a [(&'a str, &'a str)],
    /// Where Cancel goes without script.
    pub cancel_url: &'a str,
}

/// A modest inline panel, not a modal: the script moves focus to Confirm,
/// Escape and Cancel remove it.
fn confirm_panel(confirm: &Confirm<'_>) -> Markup {
    html! {
        section.confirm data-confirm aria-labelledby="confirm-title" {
            h2 #confirm-title { (confirm.action) "?" }
            p { (confirm.consequence) }
            form.inline hx-post=(confirm.post_url) hx-swap="none" data-confirm-form {
                @for (name, value) in confirm.hidden {
                    input type="hidden" name=(name) value=(value);
                }
                input type="hidden" name="confirm" value="1";
                button.btn.solid type="submit" data-confirm-yes { "Confirm" }
                a.cancel href=(confirm.cancel_url) data-confirm-cancel { "Cancel" }
            }
        }
    }
}

pub fn confirm_fragment(confirm: &Confirm<'_>) -> String {
    confirm_panel(confirm).into_string()
}

pub fn confirm_page(ctx: &Ctx<'_>, confirm: &Confirm<'_>) -> String {
    let body = html! { div #content { (confirm_panel(confirm)) } };
    layout(
        ctx,
        &format!("{} · claude-monitor", confirm.action),
        0,
        "content",
        body,
    )
    .into_string()
}

fn post_button_with(url: &str, label: &str, vals: &str) -> Markup {
    html! {
        button.btn.quiet type="button" hx-post=(url) hx-vals=(vals) hx-swap="none" { (label) }
    }
}

pub fn session(ctx: &Ctx<'_>, detail: &Value) -> String {
    let id = str_of(detail, "id").unwrap_or_default();
    let state = str_of(detail, "state").unwrap_or_default();
    let section_name = str_of(detail, "section").unwrap_or_default();
    let base = format!("/sessions/{}", seg(id));
    let flags: Vec<&str> = list_of(detail, "flags")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let parked = flags.contains(&"parked");
    let dismissed = flags.contains(&"dismissed");
    let resumed_by: Vec<&str> = list_of(detail, "resumed_by")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let resumed_from: Vec<&str> = str_of(detail, "resumed_from").into_iter().collect();
    let forked_from: Vec<&str> = str_of(detail, "forked_from").into_iter().collect();
    let generated = detail
        .get("summaries")
        .map_or(&[][..], |s| list_of(s, "generated"));
    let notes = list_of(detail, "notes");
    let waiting = usize::from(state == "needs_permission" || state == "interrupted");
    let title = str_of(detail, "task_description").unwrap_or(id);
    let inner = html! {
        h1 { (title) }
        p.state data-state=(state) { i.dot {} (state_words(state))
            @if detail.get("running").and_then(Value::as_bool) == Some(true) { " (running)" }
            @for flag in &flags { span.flag { (flag) } }
        }
        div.inline {
            @if state == "interrupted" || section_name == "finished" || section_name == "done" {
                (respawn_link(id, state == "interrupted"))
            }
            @if section_name == "finished" {
                (post_button(&format!("{base}/done"), "Mark done", false))
            }
            @if section_name == "done" {
                (post_button(&format!("{base}/undone"), "Unmark done", false))
            }
            @if parked {
                (post_button(&format!("{base}/unpark"), "Unpark", false))
            } @else {
                (post_button(&format!("{base}/park"), "Park", false))
            }
            @if dismissed {
                (post_button(&format!("{base}/undismiss"), "Undismiss", false))
            } @else {
                (confirm_button(&format!("{base}/dismiss"), "Dismiss"))
            }
        }
        dl.facts {
            dt { "Id" } dd.mono { (id) }
            dt { "Agent" } dd { (str_of(detail, "agent_kind").unwrap_or_default()) }
            @if let Some(repo) = str_of(detail, "repo") { dt { "Repo" } dd { (repo) } }
            @if let Some(branch) = str_of(detail, "branch") { dt { "Branch" } dd.mono { (branch) } }
            dt { "Directory" } dd.mono { (str_of(detail, "cwd").unwrap_or_default()) }
            dt { "tmux" }
            dd.mono {
                @if let Some(place) = placement_of(detail) {
                    (place)
                } @else {
                    span.muted { (NO_PLACEMENT) }
                }
            }
            @if let Some(project) = str_of(detail, "project") {
                dt { "Project" }
                dd { a href=(format!("/projects/{}", seg(project))) { (project) } }
            }
            dt { "Last active" }
            dd {
                (str_of(detail, "last_activity_at").map(|t| age(ctx.now, t)).unwrap_or_default())
                " ago " span.muted.mono { (str_of(detail, "last_activity_at").unwrap_or_default()) }
            }
            (lineage_links("Resumed from", &resumed_from))
            (lineage_links("Forked from", &forked_from))
            (lineage_links("Resumed by", &resumed_by))
        }
        @if let Some(task) = str_of(detail, "task_description") {
            section.panel { h2 { "Task" } p.verbatim { (task) } }
        }
        @if let Some(left) = str_of(detail, "left_off") {
            section.panel { h2 { "Left off" } p.verbatim { (left) } }
        }
        (progress_panel(id, detail))
        @for summary in generated {
            section.panel {
                h2 { (str_of(summary, "label").unwrap_or("generated")) }
                p.muted.mono {
                    (str_of(summary, "model").unwrap_or("unknown model")) ", "
                    (str_of(summary, "created_at").unwrap_or_default())
                }
                p.verbatim { (str_of(summary, "text").unwrap_or_default()) }
            }
        }
        div.panel {
            (post_button(&format!("{base}/summarize"), "Summarize with the local model", false))
        }
        (notes_panel(&format!("{base}/note"), notes))
        section.panel {
            h2 { "Project" }
            form hx-post=(format!("{base}/group")) hx-swap="none" class="inline" data-reset="" {
                label { "Move to project (- for none)" input type="text" name="project" required; }
                button.btn type="submit" { "Move" }
            }
        }
    };
    let body = html! {
        div #confirm-slot {}
        (live("content", &base, waiting, inner))
    };
    layout(
        ctx,
        &format!("{} · claude-monitor", short(id)),
        waiting,
        "content",
        body,
    )
    .into_string()
}

/// The restore preview; with `only` it is for that one session, which is how
/// a Respawn button asks first.
pub fn restore(
    ctx: &Ctx<'_>,
    preview: &Value,
    include_finished: bool,
    only: Option<&str>,
) -> String {
    let start = list_of(preview, "would_start");
    let refuse = list_of(preview, "would_refuse");
    let inner = html! {
        @if let Some(id) = only {
            h1 { "Respawn " span.mono { (short(id)) } }
        } @else {
            h1 { "Restore" }
        }
        p.muted {
            "Nothing starts until you confirm. Each session reopens as the tmux window it had before; "
            "one with no recorded window opens in tmux session "
            span.mono { (str_of(preview, "tmux_session").unwrap_or_default()) } "."
        }
        @if only.is_some() {
        } @else if include_finished {
            p { a href="/restore" { "Hide finished sessions" } }
        } @else {
            p { a href="/restore?include_finished=true" { "Include finished sessions" } }
        }
        @if start.is_empty() && refuse.is_empty() {
            p { "Nothing to restore." }
        }
        @if !start.is_empty() {
            form hx-post="/respawn" hx-swap="none" {
                ul.rows {
                    @for item in start {
                        @let id = str_of(item, "session").unwrap_or_default();
                        li.row data-state="interrupted" {
                            span.state {
                                input type="checkbox" name="ids" value=(id) checked
                                    id=(format!("pick-{id}")) aria-label=(format!("Restore {id}"));
                            }
                            div.what {
                                label for=(format!("pick-{id}")) {
                                    a.task href=(format!("/sessions/{}", seg(id))) { (short(id)) }
                                    " " span.muted { (str_of(item, "label").unwrap_or_default()) }
                                }
                                p.meta.mono {
                                    (str_of(item, "mode").unwrap_or_default()) " in "
                                    (str_of(item, "cwd").unwrap_or_default())
                                    " as tmux " (str_of(item, "tmux_target").unwrap_or_default())
                                }
                                @for notice in list_of(item, "notices") {
                                    p.warn { (notice.as_str().unwrap_or_default()) }
                                }
                            }
                            span {} span {}
                        }
                    }
                }
                @if let Some(id) = only {
                    div.inline {
                        button.btn.primary type="submit" { "Respawn" }
                        a href=(format!("/sessions/{}", seg(id))) { "Cancel" }
                    }
                } @else {
                    div.stack { button.btn.primary type="submit" { "Restore selected" } }
                }
            }
        }
        @if !refuse.is_empty() {
            section.panel {
                h2 { "Cannot be restored" }
                @for item in refuse {
                    div.warn {
                        p {
                            a.task href=(format!("/sessions/{}", seg(str_of(item, "session").unwrap_or_default()))) {
                                (short(str_of(item, "session").unwrap_or_default()))
                            }
                            " " code { (str_of(item, "code").unwrap_or_default()) }
                            ": " (str_of(item, "message").unwrap_or_default())
                        }
                        @for alternative in list_of(item, "alternatives") {
                            p.muted {
                                "Instead: " (str_of(alternative, "description").unwrap_or_default())
                                " " code { (str_of(alternative, "command").unwrap_or_default()) }
                            }
                        }
                    }
                }
            }
        }
    };
    let url = match (only, include_finished) {
        (Some(id), _) => format!("/restore?session={}", seg(id)),
        (None, true) => "/restore?include_finished=true".to_owned(),
        (None, false) => "/restore".to_owned(),
    };
    let waiting = start.len();
    let body = live("content", &url, waiting, inner);
    layout(ctx, "Restore · claude-monitor", waiting, "content", body).into_string()
}

pub fn links(ctx: &Ctx<'_>, stored: &[Value], check: Option<&Value>) -> String {
    let checked: HashMap<&str, &Value> = check
        .map(|c| list_of(c, "links"))
        .unwrap_or_default()
        .iter()
        .filter_map(|link| Some((str_of(link, "id")?, link)))
        .collect();
    let inner = html! {
        h1 { "Links" }
        @match check {
            Some(result) if str_of(result, "skipped").is_some() => {
                p.muted { "Check skipped: " (str_of(result, "skipped").unwrap_or_default()) }
            }
            Some(_) => {}
            None => {
                p { a href="/links?check=1" { "Check the links against the remote graph" } }
            }
        }
        @if stored.is_empty() {
            p.muted { "No links yet." }
        } @else {
            table {
                thead { tr { th { "Owner" } th { "Node" } th { "Kind" } th { "Last agreed" } th { "Check" } } }
                tbody {
                    @for link in stored {
                        @let id = str_of(link, "id").unwrap_or_default();
                        @let outcome = checked.get(id).and_then(|c| str_of(c, "outcome"));
                        tr.warn[outcome == Some("disagree")] {
                            td {
                                @let owner = link.get("owner").cloned().unwrap_or(Value::Null);
                                @let owner_id = str_of(&owner, "id").unwrap_or_default();
                                @if str_of(&owner, "kind") == Some("project") {
                                    a href=(format!("/projects/{}", seg(owner_id))) { (owner_id) }
                                } @else {
                                    a.mono href=(format!("/sessions/{}", seg(owner_id))) { (short(owner_id)) }
                                }
                            }
                            td.mono { (str_of(link, "node_id").unwrap_or_default()) }
                            td { (str_of(link, "kind").unwrap_or_default()) }
                            td.mono { (str_of(link, "last_agreed_at").unwrap_or("never")) }
                            td {
                                @if let Some(outcome) = outcome {
                                    (outcome.replace('_', " "))
                                    @if outcome == "disagree" {
                                        " (local "
                                        (checked.get(id).and_then(|c| str_of(c, "local_status")).unwrap_or("?"))
                                        ", remote "
                                        (checked.get(id).and_then(|c| str_of(c, "remote_status")).unwrap_or("?"))
                                        ")"
                                    }
                                } @else { span.muted { "not checked" } }
                            }
                        }
                    }
                }
            }
        }
    };
    let url = if check.is_some() {
        "/links?check=1"
    } else {
        "/links"
    };
    let body = live("content", url, 0, inner);
    layout(ctx, "Links · claude-monitor", 0, "content", body).into_string()
}

pub fn error(ctx: &Ctx<'_>, status: u16, message: &str) -> String {
    let body = html! {
        h1 { "Something stopped (" (status) ")" }
        p.warn { (message) }
        p { a href="/" { "Back to the overview" } }
    };
    layout(ctx, "claude-monitor", 0, "content", body).into_string()
}
