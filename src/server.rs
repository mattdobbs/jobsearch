//! The board's request handlers.
//!
//! The app's own window talks to these directly, in-process, through `answer`; no
//! network socket is involved. `serve` puts the same handlers behind an HTTP port
//! on this Mac only, which is used for development and automated testing.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, HOST, ORIGIN,
    REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use tower::ServiceExt;
use serde_json::{Value, json};

use crate::model::{Attachment, Job, JobDetail, Note, STATUSES, Settings};
use crate::sources::{decode_component, encode_component};
use crate::state::AppState;
use crate::{log_warn, login_item, scheduler};

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");
const FAVICON_PNG: &[u8] = include_bytes!("../assets/favicon.png");

/// Largest single attachment accepted.
const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/favicon.png", get(favicon))
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs/{id}", get(get_job).patch(move_job))
        .route("/api/jobs/{id}/notes", post(add_note))
        .route("/api/notes/{id}", put(update_note).delete(delete_note))
        .route(
            "/api/jobs/{id}/attachments",
            post(upload_attachment).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/attachments/{id}", get(download_attachment).delete(delete_attachment))
        .route("/api/attachments/{id}/open", post(open_attachment))
        .route("/api/attachments/{id}/reveal", post(reveal_attachment))
        .route("/api/status", get(status))
        .route("/api/refresh", post(refresh))
        .route("/api/settings", get(get_settings).put(put_settings))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

/// Answers one request from the app's own window.
pub async fn answer(router: Router, request: axum::http::Request<Vec<u8>>) -> axum::http::Response<Vec<u8>> {
    let (parts, body) = request.into_parts();
    let request = Request::from_parts(parts, Body::from(body));
    let response = match router.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    axum::http::Response::from_parts(parts, bytes.to_vec())
}

/// Serves the board over HTTP on this Mac only, for development and testing.
pub async fn serve(listener: std::net::TcpListener, state: Shared) {
    let port = listener.local_addr().map(|a| a.port()).unwrap_or_default();
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(e) => {
            log_warn!("could not start the web server: {e}");
            return;
        }
    };
    let app = router(state).layer(middleware::from_fn_with_state(port, guard));
    if let Err(e) = axum::serve(listener, app).await {
        log_warn!("web server stopped: {e}");
    }
}

/// Refuses HTTP requests that did not come from the board's own page.
///
/// The port is only reachable from this Mac, but any web page open in a browser can
/// still try to send requests to a local port. Checking `Host` blocks DNS rebinding;
/// checking `Origin` on anything that changes data blocks cross-site requests.
async fn guard(State(port): State<u16>, request: Request, next: Next) -> Response {
    let allowed_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host_ok = request
        .headers()
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| allowed_hosts.iter().any(|a| a == h));
    if !host_ok {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    if request.method() != Method::GET && request.method() != Method::HEAD {
        if let Some(origin) = request.headers().get(ORIGIN) {
            let origin_ok = origin
                .to_str()
                .ok()
                .and_then(|o| o.strip_prefix("http://"))
                .is_some_and(|o| allowed_hosts.iter().any(|a| a == o));
            if !origin_ok {
                return (StatusCode::FORBIDDEN, "forbidden").into_response();
            }
        }
    }
    next.run(request).await
}

/// Locks the page down: it may only load its own script, style and data. A job
/// description from the internet cannot run code or pull in anything from outside.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
             connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.entry(CACHE_CONTROL).or_insert(HeaderValue::from_static("no-store"));
    response
}

// ---- errors -----------------------------------------------------------------------

struct ApiError(StatusCode, String);

impl ApiError {
    fn not_found(what: &str) -> Self {
        ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
    }

    fn bad_request(message: impl Into<String>) -> Self {
        ApiError(StatusCode::BAD_REQUEST, message.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(error: E) -> Self {
        let error = error.into();
        log_warn!("request failed: {error:#}");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---- static assets ------------------------------------------------------------------

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(CONTENT_TYPE, content_type)], body).into_response()
}

async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn app_js() -> Response {
    asset("text/javascript; charset=utf-8", APP_JS)
}

async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

/// The browser-tab icon: the same briefcase as the app icon.
async fn favicon() -> Response {
    ([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "max-age=86400")], FAVICON_PNG).into_response()
}

// ---- jobs ---------------------------------------------------------------------------

async fn list_jobs(State(state): State<Shared>) -> ApiResult<Json<Vec<Job>>> {
    Ok(Json(state.db.list_jobs()?))
}

async fn get_job(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<JobDetail>> {
    state.db.get_job(id)?.map(Json).ok_or_else(|| ApiError::not_found("job"))
}

#[derive(Deserialize)]
struct MoveJob {
    status: String,
}

async fn move_job(
    State(state): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<MoveJob>,
) -> ApiResult<Json<Value>> {
    if !STATUSES.contains(&body.status.as_str()) {
        return Err(ApiError::bad_request(format!("unknown column: {}", body.status)));
    }
    if !state.db.set_status(id, &body.status)? {
        return Err(ApiError::not_found("job"));
    }
    state.update_badge();
    Ok(Json(json!({ "ok": true })))
}

// ---- notes --------------------------------------------------------------------------

#[derive(Deserialize)]
struct NoteBody {
    body: String,
}

fn note_text(body: NoteBody) -> ApiResult<String> {
    let text = body.body.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::bad_request("a note cannot be empty"));
    }
    Ok(text)
}

async fn add_note(
    State(state): State<Shared>,
    Path(job_id): Path<i64>,
    Json(body): Json<NoteBody>,
) -> ApiResult<Json<Note>> {
    let text = note_text(body)?;
    if !state.db.job_exists(job_id)? {
        return Err(ApiError::not_found("job"));
    }
    Ok(Json(state.db.add_note(job_id, &text)?))
}

async fn update_note(
    State(state): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<NoteBody>,
) -> ApiResult<Json<Value>> {
    let text = note_text(body)?;
    if !state.db.update_note(id, &text)? {
        return Err(ApiError::not_found("note"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_note(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    if !state.db.delete_note(id)? {
        return Err(ApiError::not_found("note"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---- attachments ------------------------------------------------------------------

/// Reduces an uploaded file name to its final component, so it can never point
/// outside the attachments folder.
fn safe_file_name(raw: &str) -> String {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    match cleaned.trim_matches('.').trim() {
        "" => "attachment".to_string(),
        _ => cleaned,
    }
}

fn attachment_path(state: &AppState, attachment: &Attachment) -> PathBuf {
    state
        .paths
        .attachments_dir
        .join(attachment.job_id.to_string())
        .join(&attachment.stored_name)
}

/// Stores one attached file. The file arrives as the raw request body, with its name
/// in the `X-File-Name` header (percent-encoded). A form upload would be the usual
/// way, but the macOS web view does not hand file form-uploads to an in-app handler.
async fn upload_attachment(
    State(state): State<Shared>,
    Path(job_id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Attachment>> {
    if !state.db.job_exists(job_id)? {
        return Err(ApiError::not_found("job"));
    }
    if body.is_empty() {
        return Err(ApiError::bad_request("that file is empty"));
    }
    let original = headers
        .get("x-file-name")
        .and_then(|value| value.to_str().ok())
        .map(|raw| safe_file_name(&decode_component(raw)))
        .unwrap_or_else(|| "attachment".to_string());

    // Each file gets a folder of its own, so it keeps its real name (and so opens
    // in the right app) without two files of the same name colliding.
    let folder = state.paths.attachments_dir.join(job_id.to_string());
    let slot = uuid::Uuid::new_v4().simple().to_string();
    tokio::fs::create_dir_all(folder.join(&slot)).await?;
    let stored_name = format!("{slot}/{original}");
    tokio::fs::write(folder.join(&stored_name), &body).await?;
    Ok(Json(state.db.add_attachment(job_id, &original, &stored_name, body.len() as i64)?))
}

async fn download_attachment(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Response> {
    let attachment = state
        .db
        .get_attachment(id)?
        .ok_or_else(|| ApiError::not_found("attachment"))?;
    let bytes = tokio::fs::read(attachment_path(&state, &attachment))
        .await
        .map_err(|_| ApiError::not_found("attachment file"))?;
    // Always a download, never rendered in the board's own origin.
    let disposition = format!(
        "attachment; filename=\"attachment\"; filename*=UTF-8''{}",
        encode_component(&attachment.filename)
    );
    let response = Response::builder()
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(CONTENT_DISPOSITION, disposition)
        .body(Body::from(bytes))?;
    Ok(response)
}

async fn delete_attachment(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let attachment = state
        .db
        .get_attachment(id)?
        .ok_or_else(|| ApiError::not_found("attachment"))?;
    state.db.delete_attachment(id)?;
    let path = attachment_path(&state, &attachment);
    if let Err(e) = tokio::fs::remove_file(&path).await {
        log_warn!("could not remove attachment file for \"{}\": {e}", attachment.filename);
    }
    // The file's own folder goes too; this fails harmlessly for older, flat files.
    if attachment.stored_name.contains('/') {
        if let Some(slot) = path.parent() {
            let _ = tokio::fs::remove_dir(slot).await;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

/// Finds an attachment's file on disk, or explains why it cannot.
fn existing_attachment(state: &AppState, id: i64) -> ApiResult<(Attachment, PathBuf)> {
    let attachment = state
        .db
        .get_attachment(id)?
        .ok_or_else(|| ApiError::not_found("attachment"))?;
    let path = attachment_path(state, &attachment);
    if !path.is_file() {
        return Err(ApiError::not_found("attachment file"));
    }
    Ok((attachment, path))
}

/// Opens an attachment in the app macOS uses for that kind of file.
async fn open_attachment(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (attachment, path) = existing_attachment(&state, id)?;
    if state.test_mode {
        crate::log_info!("test mode: would open \"{}\"", attachment.filename);
    } else {
        open::that(&path).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("could not open the file: {e}")))?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// Shows an attachment in Finder.
async fn reveal_attachment(State(state): State<Shared>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let (attachment, path) = existing_attachment(&state, id)?;
    if state.test_mode {
        crate::log_info!("test mode: would show \"{}\" in Finder", attachment.filename);
    } else {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("could not show the file: {e}")))?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---- status and settings ------------------------------------------------------------

async fn status(State(state): State<Shared>) -> ApiResult<Json<Value>> {
    let scanning = state.is_scanning();
    Ok(Json(json!({
        "scanning": scanning,
        "last_run": state.db.last_run()?,
        "next_check_at": scheduler::next_due(&state).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "interval_hours": scheduler::SCAN_INTERVAL_HOURS,
        "data_dir": state.paths.data_dir.display().to_string(),
        "employers_file": state.paths.employers_file.display().to_string(),
        "log_file": state.paths.log_file.display().to_string(),
        "start_at_login": login_item::is_enabled(),
    })))
}

async fn refresh(State(state): State<Shared>) -> Json<Value> {
    let started = state.request_scan();
    Json(json!({ "started": started, "scanning": true }))
}

async fn get_settings(State(state): State<Shared>) -> Json<Settings> {
    Json(state.db.settings())
}

/// Trims, drops blanks and removes repeats from a keyword list.
fn tidy(list: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty() && seen.insert(item.to_lowercase()))
        .collect()
}

/// Explains, in the user's terms, why these settings could never show a job.
fn describe_conflicts(settings: &Settings) -> Option<String> {
    let conflicts = crate::filter::conflicting_keywords(settings);
    if conflicts.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = conflicts
        .iter()
        .take(4)
        .map(|(role, hidden)| {
            if role.eq_ignore_ascii_case(hidden) {
                format!("\u{201c}{role}\u{201d} is in both lists.")
            } else {
                format!("\u{201c}{hidden}\u{201d} in the hide list would hide every \u{201c}{role}\u{201d} job.")
            }
        })
        .collect();
    if conflicts.len() > 4 {
        lines.push(format!("\u{2026}and {} more.", conflicts.len() - 4));
    }
    Some(format!(
        "These settings would hide the jobs you asked for. {} Remove them from the hide list; it is only for titles you do not want.",
        lines.join(" ")
    ))
}

async fn put_settings(State(state): State<Shared>, Json(incoming): Json<Settings>) -> ApiResult<Json<Settings>> {
    let settings = Settings {
        role_keywords: tidy(incoming.role_keywords),
        exclude_keywords: tidy(incoming.exclude_keywords),
        regions: tidy(incoming.regions),
        configured: true,
    };
    if let Some(problem) = describe_conflicts(&settings) {
        return Err(ApiError::bad_request(problem));
    }
    state.db.save_settings(&settings)?;
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_names_cannot_escape_the_folder() {
        assert_eq!(safe_file_name("resume.pdf"), "resume.pdf");
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\Users\\me\\cover letter.docx"), "cover letter.docx");
        assert_eq!(safe_file_name(".."), "attachment");
        assert_eq!(safe_file_name(""), "attachment");
        assert_eq!(safe_file_name("a\u{0}b.txt"), "ab.txt");
    }

    #[test]
    fn conflicting_settings_are_explained() {
        let both = Settings {
            role_keywords: vec!["devops engineer".into()],
            exclude_keywords: vec!["devops engineer".into()],
            ..Settings::default()
        };
        let message = describe_conflicts(&both).expect("a conflict");
        assert!(message.contains("\u{201c}devops engineer\u{201d} is in both lists."), "{message}");
        let fine = Settings {
            role_keywords: vec!["devops engineer".into()],
            exclude_keywords: vec!["intern".into()],
            ..Settings::default()
        };
        assert!(describe_conflicts(&fine).is_none());
    }

    #[test]
    fn keyword_lists_are_tidied() {
        let tidied = tidy(vec![" Engineer ".into(), "".into(), "engineer".into(), "QA".into()]);
        assert_eq!(tidied, vec!["Engineer".to_string(), "QA".to_string()]);
    }
}
