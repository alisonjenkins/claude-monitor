//! The pages and actions of the web view, and the table that says which
//! library command each one offers (SC-011).
//!
//! Every action runs the same library call as the CLI, so a refusal reads the
//! same in both. Reads answer JSON; pages are added on top of the same data.

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{FromRequest, OriginalUri, Path, Query, Request, State};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::pages::{self, seg};
use super::{auth, AppState};
use crate::cli::confirm::{self, Question};
use crate::cli::migrate::open_viewer_store;
use crate::cli::respawn::{plan_all, started_json, usage_error};
use crate::cli::restore::{self, candidates};
use crate::cli::show::resolve;
use crate::cli::{self, Command, Output, ProjectAction};
use crate::env::Env;
use crate::error::Error;
use crate::export::links::link_json;
use crate::model::{OwnerKind, Project, StepStatus};
use crate::respawn::{start_batch, Item, PlanFlags, RefusedItem};
use crate::store::Store;

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

const fn route(command: &'static str, method: &'static str, path: &'static str) -> CommandRoute {
    CommandRoute {
        command,
        method,
        path,
    }
}

/// Every command the web view offers. `tests/parity_web.rs` holds the commands
/// that are deliberately not here, each with its reason.
pub const COMMAND_ROUTES: &[CommandRoute] = &[
    route("list", "GET", "/sessions"),
    route("show", "GET", "/sessions/{id}"),
    route("restore", "GET", "/restore"),
    route("restore", "POST", "/respawn"),
    route("respawn", "POST", "/sessions/{id}/respawn"),
    route("respawn", "POST", "/respawn"),
    route("project", "POST", "/projects"),
    route("project", "POST", "/projects/{id}"),
    route("group", "POST", "/sessions/{id}/group"),
    route("note", "POST", "/sessions/{id}/note"),
    route("note", "POST", "/projects/{id}/note"),
    route("park", "POST", "/sessions/{id}/park"),
    route("unpark", "POST", "/sessions/{id}/unpark"),
    route("dismiss", "POST", "/sessions/{id}/dismiss"),
    route("undismiss", "POST", "/sessions/{id}/undismiss"),
    route("park", "POST", "/projects/{id}/park"),
    route("unpark", "POST", "/projects/{id}/unpark"),
    route("dismiss", "POST", "/projects/{id}/dismiss"),
    route("undismiss", "POST", "/projects/{id}/undismiss"),
    route("summarize", "POST", "/projects/{id}/summarize"),
    route("done", "POST", "/sessions/{id}/done"),
    route("undone", "POST", "/sessions/{id}/undone"),
    route("progress-bind", "POST", "/sessions/{id}/progress-source"),
    route("summarize", "POST", "/sessions/{id}/summarize"),
    route("link", "POST", "/sessions/{id}/link"),
    route("unlink", "POST", "/sessions/{id}/unlink"),
    route("links-check", "GET", "/links"),
];

const HTMX: &str = include_str!("assets/htmx.min.js");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");

/// Tells the open page that something changed, so its regions re-read.
const CHANGED: &str = "cc-changed";

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_owned(),
            message: message.into(),
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "usage", message)
    }

    fn internal(operation: &str, detail: impl std::fmt::Display) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("{operation}: {detail}"),
        )
    }

    fn unknown_project(key: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "unknown_project",
            format!("no project matches {key:?}"),
        )
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let message = error.to_string();
        match &error {
            Error::Refused { code, .. } if code.starts_with("unknown_") => {
                Self::new(StatusCode::NOT_FOUND, code, message)
            }
            Error::Refused { code, .. } => Self::new(StatusCode::CONFLICT, code, message),
            Error::Usage(_) => Self::new(StatusCode::BAD_REQUEST, "usage", message),
            Error::Config(_) => Self::new(StatusCode::BAD_REQUEST, "config", message),
            Error::NotImplemented(_) => {
                Self::new(StatusCode::NOT_IMPLEMENTED, "not_implemented", message)
            }
            Error::Partial { .. } => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "partial", message)
            }
            Error::Io { .. } | Error::Store { .. } => {
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        json_response(
            self.status,
            &json!({ "ok": false, "error": self.message, "code": self.code }),
        )
    }
}

pub fn json_response(status: StatusCode, value: &Value) -> Response {
    (
        status,
        [(CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

/// A write's answer: the JSON, plus the header that makes the page refresh.
fn changed(value: &Value) -> Response {
    let mut response = json_response(StatusCode::OK, value);
    response.headers_mut().insert(
        HeaderName::from_static("hx-trigger"),
        HeaderValue::from_static(CHANGED),
    );
    response
}

/// Runs a blocking library call off the async runtime.
async fn blocking<T, F>(job: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| ApiError::internal("run the request", e))
}

async fn run(state: &AppState, command: Command) -> Result<Output, ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    blocking(move || cli::run_in(command, &env, Some(&dir)))
        .await?
        .map_err(ApiError::from)
}

/// What the CLI prints with `--json`; `message` adds its text line for the
/// page's status line.
fn output_json(output: &Output, message: bool) -> Value {
    match output {
        Output::Dual { text, json } => {
            let mut json = json.clone();
            if let (true, Value::Object(map)) = (message, &mut json) {
                map.insert("message".to_owned(), json!(text));
            }
            json
        }
        Output::Json(value) => value.clone(),
        Output::Text(text) | Output::Raw(text) => json!({ "ok": true, "message": text }),
    }
}

async fn act(state: &AppState, command: Command) -> Result<Response, ApiError> {
    let output = run(state, command).await?;
    Ok(changed(&output_json(&output, true)))
}

/// A write's body: JSON, or the urlencoded form htmx sends, as one object. A
/// key that repeats (checkboxes) becomes an array.
#[derive(Debug, Default)]
pub struct Payload(Map<String, Value>);

impl<S: Send + Sync> FromRequest<S> for Payload {
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, ApiError> {
        let content_type = request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = Bytes::from_request(request, state)
            .await
            .map_err(|e| ApiError::usage(format!("cannot read the request body: {e}")))?;
        if body.is_empty() {
            return Ok(Self::default());
        }
        if content_type.starts_with("application/json") {
            return match serde_json::from_slice(&body) {
                Ok(Value::Object(map)) => Ok(Self(map)),
                Ok(_) => Err(ApiError::usage("the JSON body must be an object")),
                Err(e) => Err(ApiError::usage(format!("the JSON body is invalid: {e}"))),
            };
        }
        if content_type.starts_with("application/x-www-form-urlencoded") {
            return Ok(Self(form_object(&body)));
        }
        Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "usage",
            "send the body as application/json or a urlencoded form",
        ))
    }
}

fn form_object(body: &[u8]) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in form_urlencoded::parse(body) {
        let value = Value::String(value.into_owned());
        match map.get_mut(key.as_ref()) {
            Some(Value::Array(all)) => all.push(value),
            Some(first) => *first = Value::Array(vec![first.take(), value]),
            None => {
                map.insert(key.into_owned(), value);
            }
        }
    }
    map
}

impl Payload {
    fn text(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        }
    }

    fn flag(&self, key: &str) -> bool {
        match self.0.get(key) {
            Some(Value::Bool(on)) => *on,
            Some(Value::String(text)) => truthy(text),
            _ => false,
        }
    }

    /// A list given as an array, as repeated form keys, or as a bare value.
    fn list(&self, key: &str) -> Vec<String> {
        match self.0.get(key) {
            Some(Value::Array(all)) => all
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            Some(Value::String(one)) if !one.is_empty() => vec![one.clone()],
            _ => Vec::new(),
        }
    }
}

fn truthy(text: &str) -> bool {
    matches!(text.trim(), "1" | "true" | "on" | "yes")
}

fn blank(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    state: Option<String>,
    agent: Option<String>,
    repo: Option<String>,
    project: Option<String>,
    text: Option<String>,
    section: Option<String>,
    include_dismissed: Option<String>,
}

impl ListParams {
    fn filters(&self) -> pages::Filters {
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        pages::Filters {
            state: text(&self.state),
            agent: text(&self.agent),
            repo: text(&self.repo),
            project: text(&self.project),
            text: text(&self.text),
            include_dismissed: self.include_dismissed.as_deref().is_some_and(truthy),
        }
    }

    fn command(self) -> Command {
        let states = blank(self.state)
            .map(|all| all.split(',').map(|s| s.trim().to_owned()).collect())
            .unwrap_or_default();
        Command::List {
            states,
            agent: blank(self.agent),
            repo: blank(self.repo),
            project: blank(self.project),
            text: blank(self.text),
            section: blank(self.section),
            include_dismissed: self.include_dismissed.as_deref().is_some_and(truthy),
            expand_finished: false,
            expand_unseen: false,
        }
    }
}

pub fn router() -> Router<AppState> {
    let session = |action: fn(String) -> Command| {
        post(
            move |State(state): State<AppState>, Path(id): Path<String>| async move {
                act(&state, action(id)).await
            },
        )
    };
    let project = |action: fn(String) -> Command| {
        post(
            move |State(state): State<AppState>, Path(key): Path<String>| async move {
                let project = project_named(&state, key).await?;
                act(&state, action(project.name)).await
            },
        )
    };
    Router::new()
        .route("/", get(overview))
        .route(
            "/assets/htmx.min.js",
            get(|| asset("text/javascript; charset=utf-8", HTMX)),
        )
        .route(
            "/assets/app.js",
            get(|| asset("text/javascript; charset=utf-8", APP_JS)),
        )
        .route(
            "/assets/app.css",
            get(|| asset("text/css; charset=utf-8", APP_CSS)),
        )
        .route("/sessions", get(sessions))
        .route("/sessions/{id}", get(session_detail))
        .route(
            "/sessions/{id}/park",
            session(|target| Command::Park { target }),
        )
        .route(
            "/sessions/{id}/unpark",
            session(|target| Command::Unpark { target }),
        )
        .route(
            "/sessions/{id}/dismiss",
            get(session_dismiss_confirmation).post(session_dismiss),
        )
        .route(
            "/sessions/{id}/undismiss",
            session(|target| Command::Undismiss { target }),
        )
        .route(
            "/sessions/{id}/done",
            session(|id| Command::Done { sessions: vec![id] }),
        )
        .route(
            "/sessions/{id}/undone",
            session(|id| Command::Undone { sessions: vec![id] }),
        )
        .route(
            "/sessions/{id}/summarize",
            session(|target| Command::Summarize { target }),
        )
        .route("/sessions/{id}/note", post(session_note))
        .route("/sessions/{id}/group", post(session_group))
        .route("/sessions/{id}/progress-source", post(progress_source))
        .route("/sessions/{id}/link", post(link))
        .route(
            "/sessions/{id}/unlink",
            get(unlink_confirmation).post(unlink),
        )
        .route("/sessions/{id}/respawn", post(respawn_one))
        .route("/respawn", post(respawn_many))
        .route("/restore", get(restore))
        .route("/projects", get(projects).post(project_create))
        .route("/projects/{id}", get(project_detail).post(project_update))
        .route("/projects/{id}/note", post(project_note))
        .route(
            "/projects/{id}/park",
            project(|target| Command::Park { target }),
        )
        .route(
            "/projects/{id}/unpark",
            project(|target| Command::Unpark { target }),
        )
        .route(
            "/projects/{id}/dismiss",
            get(project_dismiss_confirmation).post(project_dismiss),
        )
        .route(
            "/projects/{id}/undismiss",
            project(|target| Command::Undismiss { target }),
        )
        .route(
            "/projects/{id}/summarize",
            project(|target| Command::Summarize { target }),
        )
        .route("/links", get(links))
}

fn html_response(status: StatusCode, page: String) -> Response {
    (status, [(CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

/// The page for a failed read; JSON callers get the JSON error instead.
fn settle(state: &AppState, json: bool, result: Result<Response, ApiError>) -> Response {
    match result {
        Ok(response) => response,
        Err(error) if json => error.into_response(),
        Err(error) => {
            let csrf = auth::current_csrf();
            let ctx = pages::Ctx {
                csrf: &csrf,
                now: &state.env.clock.now(),
            };
            let page = pages::error(&ctx, error.status.as_u16(), &error.message);
            html_response(error.status, page)
        }
    }
}

/// Runs `render` with the page context, answering JSON when asked.
fn page_or_json(
    state: &AppState,
    json: bool,
    data: &Value,
    render: impl FnOnce(&pages::Ctx<'_>) -> String,
) -> Response {
    if json {
        return json_response(StatusCode::OK, data);
    }
    let csrf = auth::current_csrf();
    let ctx = pages::Ctx {
        csrf: &csrf,
        now: &state.env.clock.now(),
    };
    html_response(StatusCode::OK, render(&ctx))
}

/// The stored plan figure of every session that has one, with its source.
async fn progress_figures(
    state: &AppState,
) -> Result<HashMap<String, pages::RowProgress>, ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    blocking(move || -> Result<_, ApiError> {
        let store = open_viewer_store(&env, Some(&dir))?;
        let figures = store
            .progress_figures()
            .map_err(|e| ApiError::internal("read progress figures", e))?;
        let mut out = HashMap::new();
        for (id, (done, total)) in figures {
            let source = store
                .get_progress_binding(OwnerKind::Session, &id)
                .map_err(|e| ApiError::internal("read progress binding", e))?
                .map(|binding| binding.source.label())
                .unwrap_or_default();
            out.insert(
                id,
                pages::RowProgress {
                    done,
                    total,
                    source,
                },
            );
        }
        Ok(out)
    })
    .await?
}

fn entries_of(listed: &Value) -> Vec<Value> {
    listed
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `/sessions`: the flat list, exactly what `list --json` prints.
async fn sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(params): Query<ListParams>,
) -> Response {
    let json = wants_json(&headers);
    let result = sessions_view(&state, json, false, &uri, params).await;
    settle(&state, json, result)
}

/// `/`: the same list, and as JSON also the project tree.
async fn overview(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Query(params): Query<ListParams>,
) -> Response {
    let json = wants_json(&headers);
    let result = sessions_view(&state, json, true, &uri, params).await;
    settle(&state, json, result)
}

/// Every project, and the plan figure of those that have one.
async fn projects_with_progress(
    state: &AppState,
) -> Result<(Vec<Project>, HashMap<String, pages::RowProgress>), ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    blocking(move || -> Result<_, ApiError> {
        let store = open_viewer_store(&env, Some(&dir))?;
        let projects = store
            .list_projects()
            .map_err(|e| ApiError::internal("list projects", e))?;
        let mut figures = HashMap::new();
        for project in &projects {
            let Some(binding_id) = project.progress_binding_id.as_deref() else {
                continue;
            };
            let steps = store
                .list_steps(binding_id)
                .map_err(|e| ApiError::internal("read project steps", e))?;
            if steps.is_empty() {
                continue;
            }
            let source = store
                .get_progress_binding(OwnerKind::Project, &project.id)
                .map_err(|e| ApiError::internal("read progress binding", e))?
                .map(|binding| binding.source.label())
                .unwrap_or_default();
            let done = steps
                .iter()
                .filter(|step| step.status == StepStatus::Done)
                .count();
            figures.insert(
                project.id.clone(),
                pages::RowProgress {
                    done: u32::try_from(done).unwrap_or(u32::MAX),
                    total: u32::try_from(steps.len()).unwrap_or(u32::MAX),
                    source,
                },
            );
        }
        Ok((projects, figures))
    })
    .await?
}

async fn sessions_view(
    state: &AppState,
    json: bool,
    tree: bool,
    uri: &Uri,
    params: ListParams,
) -> Result<Response, ApiError> {
    let filters = params.filters();
    let output = run(state, params.command()).await?;
    let mut listed = output_json(&output, false);
    let entries = entries_of(&listed);
    let grown = if json && !tree {
        Value::Null
    } else {
        project_tree(state, &entries).await?
    };
    if json {
        if let (Some(target), Value::Object(grown)) = (listed.as_object_mut(), grown.clone()) {
            target.extend(grown);
        }
    }
    let progress = if json {
        HashMap::new()
    } else {
        progress_figures(state).await?
    };
    let url = uri.path_and_query().map_or("/", |pq| pq.as_str());
    Ok(page_or_json(state, json, &listed, |ctx| {
        pages::overview(ctx, &grown, &entries, &progress, &filters, url)
    }))
}

/// Every project with the listed sessions nested under it.
async fn project_tree(state: &AppState, entries: &[Value]) -> Result<Value, ApiError> {
    let (projects, figures) = projects_with_progress(state).await?;
    let projects = json!(projects).as_array().cloned().unwrap_or_default();
    Ok(super::tree::build(&projects, entries, &figures))
}

async fn session_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let json = wants_json(&headers);
    let result = async {
        let output = run(&state, Command::Show { session: id }).await?;
        let detail = output_json(&output, false);
        Ok(page_or_json(&state, json, &detail, |ctx| {
            pages::session(ctx, &detail)
        }))
    }
    .await;
    settle(&state, json, result)
}

async fn session_note(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let text = body.text("text").unwrap_or_default();
    act(&state, Command::Note { target: id, text }).await
}

async fn session_group(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let project = blank(body.text("project"))
        .ok_or_else(|| ApiError::usage("name the project, or \"-\" for none"))?;
    act(
        &state,
        Command::Group {
            project,
            sessions: vec![id],
        },
    )
    .await
}

async fn progress_source(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let (file, source) = (
        blank(body.text("path")).map(PathBuf::from),
        blank(body.text("source")),
    );
    act(
        &state,
        Command::ProgressBind {
            owner: id,
            file,
            source,
        },
    )
    .await
}

async fn link(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let cortex = blank(body.text("cortex")).unwrap_or_default();
    let kind = blank(body.text("kind")).unwrap_or_else(|| "task".to_owned());
    let doc = blank(body.text("doc"));
    act(
        &state,
        Command::Link {
            owner: id,
            cortex,
            kind,
            doc,
        },
    )
    .await
}

async fn unlink(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let cortex = blank(body.text("cortex")).unwrap_or_default();
    confirmed_act(
        &state,
        Command::Unlink {
            owner: id,
            cortex,
            yes: false,
        },
        &body,
    )
    .await
}

#[derive(Debug, Default, Deserialize)]
pub struct UnlinkParams {
    cortex: Option<String>,
}

async fn unlink_confirmation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<UnlinkParams>,
) -> Response {
    let cortex = blank(params.cortex).unwrap_or_default();
    let base = format!("/sessions/{}", seg(&id));
    let command = Command::Unlink {
        owner: id,
        cortex: cortex.clone(),
        yes: false,
    };
    confirmation(
        &state,
        &headers,
        command,
        &format!("{base}/unlink"),
        &[("cortex", cortex.as_str())],
        &base,
    )
    .await
}

async fn session_dismiss(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    confirmed_act(
        &state,
        Command::Dismiss {
            target: id,
            yes: false,
        },
        &body,
    )
    .await
}

async fn session_dismiss_confirmation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let base = format!("/sessions/{}", seg(&id));
    let command = Command::Dismiss {
        target: id,
        yes: false,
    };
    confirmation(
        &state,
        &headers,
        command,
        &format!("{base}/dismiss"),
        &[],
        &base,
    )
    .await
}

async fn project_dismiss(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let project = project_named(&state, key).await?;
    confirmed_act(
        &state,
        Command::Dismiss {
            target: project.name,
            yes: false,
        },
        &body,
    )
    .await
}

async fn project_dismiss_confirmation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    let json = wants_json(&headers);
    let project = match project_named(&state, key).await {
        Ok(project) => project,
        Err(error) => return settle(&state, json, Err(error)),
    };
    let base = format!("/projects/{}", seg(&project.id));
    let command = Command::Dismiss {
        target: project.name,
        yes: false,
    };
    confirmation(
        &state,
        &headers,
        command,
        &format!("{base}/dismiss"),
        &[],
        &base,
    )
    .await
}

/// What the confirmation asks, or the error for an unknown target.
async fn question_of(state: &AppState, command: Command) -> Result<Question, ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    blocking(move || confirm::question_for(&command, &env, Some(&dir)))
        .await?
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("confirm", "the command needs no confirmation"))
}

/// Runs a command that needs confirmation only when the body carries
/// `confirm`; otherwise 409 `confirmation_required` and nothing changes. An
/// unknown target is still a 404.
async fn confirmed_act(
    state: &AppState,
    command: Command,
    body: &Payload,
) -> Result<Response, ApiError> {
    if body.flag("confirm") {
        return act(state, confirm::confirmed(command)).await;
    }
    let question = question_of(state, command).await?;
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "confirmation_required",
        format!("{question}; send confirm=1 to proceed"),
    ))
}

/// The confirmation a person sees before the change: JSON for scripts, a
/// fragment for htmx (shown inline), a whole page otherwise. Changes nothing.
async fn confirmation(
    state: &AppState,
    headers: &HeaderMap,
    command: Command,
    post_url: &str,
    hidden: &[(&str, &str)],
    cancel_url: &str,
) -> Response {
    let json = wants_json(headers);
    let result = async {
        let question = question_of(state, command).await?;
        if json {
            return Ok(json_response(
                StatusCode::OK,
                &json!({
                    "ok": true,
                    "confirmation_required": true,
                    "action": question.action,
                    "consequence": question.consequence,
                    "confirm_with": { "method": "POST", "url": post_url, "confirm": 1 },
                }),
            ));
        }
        let panel = pages::Confirm {
            action: &question.action,
            consequence: &question.consequence,
            post_url,
            hidden,
            cancel_url,
        };
        let page = if headers.contains_key("hx-request") {
            pages::confirm_fragment(&panel)
        } else {
            let csrf = auth::current_csrf();
            let ctx = pages::Ctx {
                csrf: &csrf,
                now: &state.env.clock.now(),
            };
            pages::confirm_page(&ctx, &panel)
        };
        Ok(html_response(StatusCode::OK, page))
    }
    .await;
    settle(state, json, result)
}

#[derive(Debug, Deserialize)]
pub struct CheckParams {
    check: Option<String>,
}

async fn links(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CheckParams>,
) -> Response {
    let json = wants_json(&headers);
    let result = links_view(&state, json, &params).await;
    settle(&state, json, result)
}

async fn links_view(
    state: &AppState,
    json: bool,
    params: &CheckParams,
) -> Result<Response, ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    let stored = blocking(move || -> Result<Vec<Value>, Error> {
        let store = open_viewer_store(&env, Some(&dir))?;
        let all = store.all_links().map_err(|e| Error::Store {
            operation: "list links".to_owned(),
            path: None,
            detail: e.to_string(),
        })?;
        Ok(all.iter().map(link_json).collect())
    })
    .await??;
    let mut body = Map::new();
    body.insert("links".to_owned(), json!(stored));
    if params.check.as_deref().is_some_and(truthy) {
        let output = run(state, Command::LinksCheck).await?;
        body.insert("check".to_owned(), output_json(&output, false));
    }
    let body = Value::Object(body);
    Ok(page_or_json(state, json, &body, |ctx| {
        pages::links(ctx, &stored, body.get("check"))
    }))
}

fn find_project(store: &Store, key: &str) -> Result<Project, ApiError> {
    store
        .list_projects()
        .map_err(|e| ApiError::internal("list projects", e))?
        .into_iter()
        .find(|p| p.id == key || p.name == key)
        .ok_or_else(|| ApiError::unknown_project(key))
}

/// The project list is part of the overview now; scripts still get the JSON.
async fn projects(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let json = wants_json(&headers);
    if !json {
        return (StatusCode::FOUND, [(LOCATION, "/")]).into_response();
    }
    let result = projects_with_progress(&state)
        .await
        .map(|(all, _)| json_response(StatusCode::OK, &json!({ "projects": all })));
    settle(&state, json, result)
}

fn entries_named(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

async fn project_create(
    State(state): State<AppState>,
    body: Payload,
) -> Result<Response, ApiError> {
    let name = blank(body.text("name")).ok_or_else(|| ApiError::usage("a project needs a name"))?;
    act(
        &state,
        Command::Project {
            action: ProjectAction::Add,
            name,
            value: None,
        },
    )
    .await
}

async fn project_named(state: &AppState, key: String) -> Result<Project, ApiError> {
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    blocking(move || {
        let store = open_viewer_store(&env, Some(&dir))?;
        find_project(&store, &key)
    })
    .await?
}

async fn project_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(key): Path<String>,
) -> Response {
    let json = wants_json(&headers);
    let result = project_view(&state, json, &uri, key).await;
    settle(&state, json, result)
}

async fn project_view(
    state: &AppState,
    json: bool,
    uri: &Uri,
    key: String,
) -> Result<Response, ApiError> {
    let project = project_named(state, key).await?;
    let notes = {
        let (env, dir, id) = (
            state.env.clone(),
            state.state_dir.clone(),
            project.id.clone(),
        );
        blocking(move || -> Result<Value, ApiError> {
            let store = open_viewer_store(&env, Some(&dir))?;
            let notes = store
                .notes_for(OwnerKind::Project, &id)
                .map_err(|e| ApiError::internal("read notes", e))?;
            Ok(json!(notes))
        })
        .await??
    };
    let listed = run(
        state,
        ListParams {
            project: Some(project.name.clone()),
            ..ListParams::default()
        }
        .command(),
    )
    .await?;
    let entries = entries_of(&output_json(&listed, false));
    let body = json!({ "project": project, "sessions": entries, "notes": notes });
    let progress = if json {
        HashMap::new()
    } else {
        progress_figures(state).await?
    };
    let url = uri.path_and_query().map_or("/", |pq| pq.as_str());
    let tree = if json {
        Value::Null
    } else {
        project_tree(state, &entries).await?
    };
    Ok(page_or_json(state, json, &body, |ctx| {
        pages::project(
            ctx,
            &json!(project),
            &entries_named(&body, "notes"),
            &tree,
            &entries,
            &progress,
            url,
        )
    }))
}

async fn project_update(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let mut name = project_named(&state, key).await?.name;
    let (rename, status, sessions) = (
        blank(body.text("name")),
        blank(body.text("status")),
        body.list("sessions"),
    );
    if rename.is_none() && status.is_none() && sessions.is_empty() {
        return Err(ApiError::usage(
            "send a new name, a status or sessions to group",
        ));
    }
    let mut last = None;
    if let Some(new_name) = rename {
        let command = Command::Project {
            action: ProjectAction::Rename,
            name: name.clone(),
            value: Some(new_name.clone()),
        };
        last = Some(run(&state, command).await?);
        name = new_name;
    }
    if let Some(status) = status {
        let command = Command::Project {
            action: ProjectAction::SetStatus,
            name: name.clone(),
            value: Some(status),
        };
        last = Some(run(&state, command).await?);
    }
    if !sessions.is_empty() {
        let command = Command::Group {
            project: name,
            sessions,
        };
        last = Some(run(&state, command).await?);
    }
    let output = last.ok_or_else(|| ApiError::usage("nothing to change"))?;
    Ok(changed(&output_json(&output, true)))
}

async fn project_note(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let project = project_named(&state, key).await?;
    let text = body.text("text").unwrap_or_default();
    act(
        &state,
        Command::Note {
            target: project.name,
            text,
        },
    )
    .await
}

fn refused_json(item: &RefusedItem) -> Value {
    json!({
        "id": item.session_id,
        "code": item.code,
        "message": item.message,
        "alternatives": item.alternatives.iter().map(|a| json!({
            "description": a.description,
            "command": a.command,
        })).collect::<Vec<_>>(),
    })
}

/// The outcome of a batch: `started` and `refused` are both data, so a
/// refusal is a 200.
fn batch_json(items: &[Item], skipped: &[String]) -> Value {
    let refused: Vec<Value> = items
        .iter()
        .filter_map(|item| match item {
            Item::Refused(r) => Some(refused_json(r)),
            Item::Started(_) => None,
        })
        .collect();
    let started = started_json(items);
    json!({
        "ok": true,
        "executed": true,
        "message": format!("started {}, refused {}", started.len(), refused.len()),
        "started": started,
        "refused": refused,
        "skipped": skipped,
    })
}

/// Plans and starts `ids` with `flags`, the way `respawn` and `restore` do.
fn start_sessions(
    store: &Store,
    env: &Env,
    ids: Vec<String>,
    skipped: &[String],
    flags: PlanFlags,
) -> Result<Value, Error> {
    if ids.is_empty() {
        return Ok(batch_json(&[], skipped));
    }
    let planned = plan_all(store, env, &ids, &flags)?;
    if let Some(error) = usage_error(&planned) {
        return Err(error);
    }
    let items = start_batch(store, env, planned);
    Ok(batch_json(&items, skipped))
}

fn flags_of(body: &Payload) -> PlanFlags {
    PlanFlags {
        fresh: body.flag("fresh"),
        in_dir: blank(body.text("in")).map(PathBuf::from),
        recreate_worktree: body.flag("recreate_worktree"),
    }
}

async fn respawn_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Payload,
) -> Result<Response, ApiError> {
    let flags = flags_of(&body);
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    let report = blocking(move || -> Result<Value, Error> {
        let store = open_viewer_store(&env, Some(&dir))?;
        let id = resolve(&store, &id)?;
        start_sessions(&store, &env, vec![id], &[], flags)
    })
    .await??;
    Ok(changed(&report))
}

async fn respawn_many(State(state): State<AppState>, body: Payload) -> Result<Response, ApiError> {
    let flags = flags_of(&body);
    let ids = body.list("ids");
    let all_interrupted = body.flag("all_interrupted");
    let include_finished = body.flag("include_finished");
    let except = body.list("except");
    if ids.is_empty() && !all_interrupted {
        return Err(ApiError::usage(
            "name the sessions in \"ids\", or set \"all_interrupted\"",
        ));
    }
    let (env, dir) = (state.env.clone(), state.state_dir.clone());
    let report = blocking(move || -> Result<Value, Error> {
        let store = open_viewer_store(&env, Some(&dir))?;
        let mut skipped = Vec::new();
        for id in &except {
            let id = resolve(&store, id)?;
            if !skipped.contains(&id) {
                skipped.push(id);
            }
        }
        let named: Vec<String> = if all_interrupted {
            candidates(&store, include_finished)?
                .into_iter()
                .map(|s| s.id)
                .collect()
        } else {
            ids.iter()
                .map(|id| resolve(&store, id))
                .collect::<Result<_, _>>()?
        };
        let mut chosen = Vec::new();
        for id in named {
            if !skipped.contains(&id) && !chosen.contains(&id) {
                chosen.push(id);
            }
        }
        start_sessions(&store, &env, chosen, &skipped, flags)
    })
    .await??;
    Ok(changed(&report))
}

#[derive(Debug, Default, Deserialize)]
pub struct RestoreParams {
    include_finished: Option<String>,
    /// Preview this one session instead of every interrupted one.
    session: Option<String>,
}

async fn restore(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RestoreParams>,
) -> Response {
    let json = wants_json(&headers);
    let include_finished = params.include_finished.as_deref().is_some_and(truthy);
    let only = blank(params.session);
    let result = async {
        let output = match &only {
            Some(session) => {
                let (env, dir, session) =
                    (state.env.clone(), state.state_dir.clone(), session.clone());
                blocking(move || restore::preview_session(&env, Some(&dir), &session))
                    .await?
                    .map_err(ApiError::from)?
            }
            None => {
                run(
                    &state,
                    Command::Restore {
                        all_interrupted: false,
                        include_finished,
                        fresh: false,
                        skip: Vec::new(),
                        yes: false,
                    },
                )
                .await?
            }
        };
        let preview = output_json(&output, false);
        Ok(page_or_json(&state, json, &preview, |ctx| {
            pages::restore(ctx, &preview, include_finished, only.as_deref())
        }))
    }
    .await;
    settle(&state, json, result)
}

async fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(CONTENT_TYPE, content_type)], body).into_response()
}

/// Whether the caller asked for JSON rather than a page.
pub fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"))
}
