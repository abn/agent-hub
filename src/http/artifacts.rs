//! Artifact REST routes: the project listing and the public viewer.
//!
//! Agent-authored content is untrusted. The public viewer splits into two
//! documents. The host shell at `/artifacts/{id}` carries the title, the
//! version picker data, and JSON blobs the first-party viewer module reads;
//! it never carries author bytes. The frame at `/artifacts/{id}/frame`
//! serves an HTML artifact's bytes verbatim inside a sandboxed document
//! whose policy names the request origin. A markdown artifact renders in the
//! host page from the inlined source, so the frame refuses it. A protected
//! artifact has no plaintext on the server: its host shell carries only the
//! envelope and the ciphertext for the browser to decrypt, with no picker
//! and no body bytes.

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::origin::request_origin;
use crate::http::problem::{Problem, ProblemPath, ProblemQuery, json_body};
use crate::markdown::escape_html;
use crate::store::artifacts::{self as artifact_store, Artifact, ArtifactVersion};
use crate::store::comments::{self as comment_store, Comment, comment_view, parse_anchor};

/// The artifacts of one project.
#[derive(Debug, Serialize)]
pub struct ArtifactList {
    /// The artifacts, most recently updated first.
    pub artifacts: Vec<Artifact>,
}

/// Query parameters for listing artifacts.
#[derive(Debug, Default, Deserialize)]
pub struct ArtifactListQuery {
    /// Filter to artifacts published during this session.
    pub session: Option<String>,
    /// Alias for `session`.
    pub session_id: Option<String>,
}

impl ArtifactListQuery {
    fn session_filter(&self) -> Option<&str> {
        self.session.as_deref().or(self.session_id.as_deref())
    }
}

/// `GET /api/v1/artifacts`
///
/// A valid bearer token is required. Filtered by `?session=<id>`.
pub async fn list_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<ArtifactListQuery>,
) -> std::result::Result<Json<ArtifactList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let artifacts = match query.session_filter() {
        Some(session_id) => artifact_store::list_for_session(&state.db, session_id).await,
        None => {
            return Err(Problem::from_error(&Error::InvalidArgument(
                "session query parameter is required".to_string(),
            )));
        }
    }
    .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(ArtifactList { artifacts }))
}

/// `GET /api/v1/projects/{id}/artifacts`
///
/// A valid bearer token is required. Optionally filtered by `?session=<id>`.
pub async fn list(
    State(state): State<AppState>,
    ProblemPath(project_id): ProblemPath<String>,
    headers: HeaderMap,
    ProblemQuery(query): ProblemQuery<ArtifactListQuery>,
) -> std::result::Result<Json<ArtifactList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let artifacts =
        artifact_store::list_with_session(&state.db, &project_id, query.session_filter())
            .await
            .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(ArtifactList { artifacts }))
}

/// The content and encryption envelope of one artifact, for the browser
/// decryptor. The server holds no plaintext for a protected artifact.
#[derive(Debug, Serialize)]
pub struct ArtifactContent {
    /// Artifact actor who created it.
    pub actor: Option<String>,
    /// Artifact title.
    pub title: String,
    /// `html` or `markdown`.
    pub kind: String,
    /// Whether the content is encrypted.
    pub protected: bool,
    /// The encryption envelope, when protected.
    pub envelope: Option<serde_json::Value>,
    /// The content, as the stored UTF-8 text; a ciphertext arrives base64.
    pub content: String,
    /// The version the content was read at.
    pub version: i64,
    /// Artifact description.
    pub description: String,
    /// The label of the version read, when set.
    pub label: Option<String>,
    /// Total comments on the artifact across all versions.
    pub comments_count: i64,
    /// Comments not marked done across all versions.
    pub comments_open: i64,
}

/// The `?version=N` selector shared by the versioned artifact routes.
#[derive(Debug, Default, Deserialize)]
pub struct VersionQuery {
    /// The version to read. Omitted, the current version is read.
    pub version: Option<i64>,
}

/// The `?version=N&pass=...` selector of the public host page.
#[derive(Debug, Default, Deserialize)]
pub struct HostQuery {
    /// The version to read. Omitted, the current version is read.
    pub version: Option<i64>,
    /// The owner pass, which reads a page an active share conceals.
    pub pass: Option<String>,
}

/// The `?version=N&theme=light|dark&pass=...` selector of the frame route.
#[derive(Debug, Default, Deserialize)]
pub struct FrameQuery {
    /// The version to read. Omitted, the current version is read.
    pub version: Option<i64>,
    /// The frame theme. Only `dark` selects dark; anything else is light.
    pub theme: Option<String>,
    /// The owner pass, which reads a body an active share conceals.
    pub pass: Option<String>,
}

/// The version history of one artifact, oldest first.
#[derive(Debug, Serialize)]
pub struct VersionList {
    /// The versions, oldest first.
    pub versions: Vec<ArtifactVersion>,
}

/// The acknowledgement returned when an artifact is deleted.
#[derive(Debug, Serialize)]
pub struct DestroyResult {
    /// Always true on success.
    pub ok: bool,
}

/// `GET /api/v1/artifacts/{id}`
///
/// Admin-only. Returns the content for the in-app viewer and decryptor.
/// With `?version=N`, returns that version instead of the current one.
pub async fn content(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<VersionQuery>,
    headers: HeaderMap,
) -> std::result::Result<Json<ArtifactContent>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let (artifact, bytes) = match query.version {
        Some(version) => {
            artifact_store::get_at_version(&state.db, &state.data_dir, &artifact_id, version)
                .await
                .map_err(|err| Problem::from_error(&err))?
        }
        None => artifact_store::get(&state.db, &state.data_dir, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?,
    };

    let content = String::from_utf8(bytes).map_err(|_| {
        Problem::from_error(&Error::InvalidArgument(format!(
            "artifact {artifact_id} content is not UTF-8 text"
        )))
    })?;

    Ok(Json(ArtifactContent {
        actor: artifact.actor,
        title: artifact.title,
        kind: artifact.kind,
        protected: artifact.protected,
        envelope: artifact.envelope,
        content,
        version: artifact.version,
        description: artifact.description,
        label: artifact.label,
        comments_count: artifact.comments_count,
        comments_open: artifact.comments_open,
    }))
}

/// `GET /api/v1/artifacts/{id}/versions`
///
/// Admin-only. Returns the version history, oldest first.
pub async fn versions(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<VersionList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let versions = artifact_store::list_versions(&state.db, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(VersionList { versions }))
}

/// The comment to post on an artifact.
#[derive(Debug, Deserialize)]
pub struct CommentPostBody {
    /// The comment text.
    pub body: String,
    /// Optional anchor: `{mode: "point", x, y}` or `{mode: "text", quote}`.
    #[serde(default)]
    pub anchor: Option<Value>,
    /// Optional anchored version; omitted stamps the current version.
    #[serde(default)]
    pub anchor_version: Option<i64>,
    /// Optional idempotency key, so a retried post returns the original.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// The resolution flip for a comment.
#[derive(Debug, Deserialize)]
pub struct CommentResolveBody {
    /// Whether the comment is done.
    pub done: bool,
}

/// `GET /api/v1/artifacts/{id}/comments`
///
/// Admin-only. Returns the artifact's comments, oldest first.
pub async fn comment_list(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let listed = comment_store::list_comments(&state.db, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(json!({
        "comments": listed.iter().map(comment_view).collect::<Vec<_>>(),
    })))
}

/// `POST /api/v1/artifacts/{id}/comments`
///
/// Admin-only. Authors the comment as `"human"` and stores no delete token;
/// later mutations go through the admin gate alone. A replayed idempotency
/// key returns the recorded comment.
pub async fn comment_post(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentPostBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "comment body must be JSON with a body field")?;
    let anchor = parse_anchor(payload.anchor).map_err(|err| Problem::from_error(&err))?;
    let (comment, _replayed) = comment_store::add_comment(
        &state.db,
        &artifact_id,
        "human",
        &payload.body,
        anchor,
        payload.anchor_version,
        None,
        payload.idempotency_key.as_deref(),
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(comment_view(&comment)))
}

/// `PATCH /api/v1/artifacts/{id}/comments/{commentId}`
///
/// Admin-only. Flips the resolution of one comment. A comment of another
/// artifact, like an unknown id, is a 404.
pub async fn comment_resolve(
    State(state): State<AppState>,
    ProblemPath((artifact_id, comment_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentResolveBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let payload = json_body(body, "resolve body must be JSON with a done field")?;
    let comment = comment_store::get_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    if comment.artifact_id != artifact_id {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "comment {comment_id} not found"
        ))));
    }
    let updated = comment_store::set_comment_done(&state.db, &comment_id, payload.done)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(comment_view(&updated)))
}

/// `DELETE /api/v1/artifacts/{id}/comments/{commentId}`
///
/// Admin-only. Removes one comment. A comment of another artifact, like an
/// unknown id, is a 404.
pub async fn comment_remove(
    State(state): State<AppState>,
    ProblemPath((artifact_id, comment_id)): ProblemPath<(String, String)>,
    headers: HeaderMap,
) -> std::result::Result<Json<DestroyResult>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let comment = comment_store::get_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    if comment.artifact_id != artifact_id {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "comment {comment_id} not found"
        ))));
    }
    comment_store::delete_comment(&state.db, &comment_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(DestroyResult { ok: true }))
}

/// `DELETE /api/v1/artifacts/{id}`
///
/// Admin-only. Deletes the artifact and its history.
pub async fn destroy(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<DestroyResult>, Problem> {
    let principal = state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    artifact_store::delete(&state.db, &state.data_dir, &principal.actor, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    state.notify();
    Ok(Json(DestroyResult { ok: true }))
}

/// `GET /api/v1/artifacts/{id}/raw`
///
/// Admin-only. Returns the raw bytes of the current version, or of
/// `?version=N` when given. A public artifact arrives as text; a protected
/// one arrives as a JSON envelope plus base64 ciphertext for the browser to
/// decrypt.
pub async fn raw(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<VersionQuery>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let (artifact, bytes) = match query.version {
        Some(version) => {
            artifact_store::get_at_version(&state.db, &state.data_dir, &artifact_id, version)
                .await
                .map_err(|err| Problem::from_error(&err))?
        }
        None => artifact_store::get(&state.db, &state.data_dir, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?,
    };

    if artifact.protected {
        // The stored bytes are the client's base64 ciphertext text, so they
        // travel verbatim: encoding them again would break the browser
        // decryptor, which decodes exactly once.
        let ciphertext = String::from_utf8(bytes).map_err(|_| {
            Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {artifact_id} ciphertext is not UTF-8 text"
            )))
        })?;
        let body = serde_json::json!({
            "envelope": artifact.envelope,
            "ciphertext": ciphertext,
        });
        Ok(raw_json_response(body.to_string()))
    } else {
        Ok(raw_text_response(bytes))
    }
}

/// A viewer pass for one artifact, as the app embeds its page.
#[derive(Debug, Serialize)]
pub struct ViewerPassResponse {
    /// The pass, or empty when this artifact's page needs none.
    pub pass: String,
}

/// The width of a pass's window in seconds. A pass is recomputed when it is
/// checked rather than stored, so a window is how long one stays open.
const VIEWER_PASS_WINDOW: u64 = 60;

/// How many windows a pass is accepted in, its own included. Two covers a frame
/// that loads the host page and the host page's own inner frame a moment later,
/// and keeps a pass in a history entry worthless within a couple of minutes.
const VIEWER_PASS_WINDOWS: u64 = 2;

/// The window this request falls in.
fn pass_window() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() / VIEWER_PASS_WINDOW)
        .unwrap_or(0)
}

/// The pass for one artifact in one window.
///
/// A keyed digest rather than a stored row: nothing to keep, nothing to expire
/// in a background pass, and nothing to garbage collect. The key is the admin
/// token, so a pass cannot be computed without it, and the artifact id is in
/// the digest, so a pass for one artifact reads no other. Sixteen bytes of
/// digest is a value nobody guesses.
fn derive_viewer_pass(secret: &str, artifact_id: &str, window: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [secret, artifact_id, &window.to_string()] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether a presented value is a live pass for this artifact.
fn viewer_pass_holds(state: &AppState, artifact_id: &str, presented: Option<&str>) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    // A hub with no admin token has no control surface to mint a pass from, so
    // there is nothing for a pass to stand in for.
    let Some(secret) = state.config.admin_token.as_deref() else {
        return false;
    };
    let current = pass_window();
    (0..VIEWER_PASS_WINDOWS).any(|back| {
        // Compared as a value equality on a digest rather than byte by byte:
        // the pass is not the admin token, and a wrong one is refused on the
        // first differing byte.
        presented == derive_viewer_pass(secret, artifact_id, current.saturating_sub(back))
    })
}

/// Whether this caller must not see this artifact on the public routes.
///
/// A share link is the only way in to a plain artifact that has one, so every
/// other caller gets the same 404 an unknown artifact gets. The owner's own
/// viewer is not that caller: it reads the page with a pass, because an iframe
/// navigation cannot carry the bearer token the app reads everything else with.
async fn concealed(
    state: &AppState,
    artifact: &Artifact,
    pass: Option<&str>,
) -> std::result::Result<bool, Problem> {
    if artifact.protected || viewer_pass_holds(state, &artifact.id, pass) {
        return Ok(false);
    }
    artifact_store::has_active_share(&state.db, &artifact.id)
        .await
        .map_err(|err| Problem::from_error(&err))
}

/// `GET /api/v1/artifacts/{id}/viewer-pass`
///
/// Admin only. The pass the app needs to read this artifact's public page, which
/// it embeds in a frame. An iframe navigation carries no bearer token, so a page
/// an active share conceals is otherwise unreachable from the owner's own
/// viewer. An artifact whose page is public anyway answers an empty pass, so the
/// address a reader ends up with carries a credential only where one is needed.
/// A pass reads the one artifact it was minted for, for as long as its window
/// lasts, and is not the share token: it grants nothing a recipient of a share
/// link does not already have, and opens nothing the admin token does not
/// already open.
pub async fn viewer_pass(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<ViewerPassResponse>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let artifact = artifact_store::metadata(&state.db, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    // A protected artifact's page is never concealed: the password is the gate.
    // So is a plain one nobody has shared. Either way there is nothing to pass,
    // and no reason to put a credential in a frame address.
    if artifact.protected || !artifact_store::has_active_share(&state.db, &artifact_id).await.map_err(
        |err| Problem::from_error(&err),
    )? {
        return Ok(Json(ViewerPassResponse {
            pass: String::new(),
        }));
    }

    let secret = state.config.admin_token.as_deref().ok_or_else(|| {
        Problem::from_error(&Error::InvalidArgument(
            "no admin token is configured, so no pass can be minted".to_string(),
        ))
    })?;
    Ok(Json(ViewerPassResponse {
        pass: derive_viewer_pass(secret, &artifact_id, pass_window()),
    }))
}

/// `GET /artifacts/{id}`
///
/// Public: a recipient opens the link without a token. The host shell carries
/// no author bytes: a plain HTML artifact is viewed through the frame route,
/// a plain markdown artifact renders in the host page from the inlined
/// source, and a protected artifact shows the unlock form with the envelope
/// and ciphertext. With `?version=N`, serves that version instead of the
/// current one, and `?pass=` is a pass from the viewer-pass route, which is how
/// the app reads a page an active share conceals. All logic lives in the viewer
/// module and the vendor scripts; the shell itself has no inline scripts.
pub async fn host(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<HostQuery>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    let (artifact, bytes) = match query.version {
        Some(version) => {
            artifact_store::get_at_version(&state.db, &state.data_dir, &artifact_id, version)
                .await
                .map_err(|err| Problem::from_error(&err))?
        }
        None => artifact_store::get(&state.db, &state.data_dir, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?,
    };

    if concealed(&state, &artifact, query.pass.as_deref()).await? {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "artifact {artifact_id} not found"
        ))));
    }

    let shown = query.version.unwrap_or(artifact.version);
    let pinned = query.version.is_some();
    let origin = request_origin(&state.config, &headers);

    let document = if artifact.protected {
        // The stored bytes are the client's base64 ciphertext text; they
        // travel verbatim so the browser decryptor decodes exactly once.
        let ciphertext = String::from_utf8(bytes).map_err(|_| {
            Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {artifact_id} ciphertext is not UTF-8 text"
            )))
        })?;
        locked_shell(&artifact, &ciphertext, shown, pinned, &origin, None)
    } else {
        let versions = artifact_store::list_versions(&state.db, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        let thread = comment_store::list_comments(&state.db, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        reader_shell(
            &artifact,
            &bytes,
            shown,
            pinned,
            &versions,
            &thread,
            &origin,
            None,
            query.pass.as_deref(),
        )
    };
    Ok(host_response(document))
}

/// `GET /artifacts/{id}/frame`
///
/// Public: the sandboxed body of a plain HTML artifact, served verbatim. A
/// markdown artifact renders in the host page, so this route refuses it with
/// `invalid_argument`; a protected artifact has no servable plaintext and is
/// refused the same way. With `?version=N`, serves that version instead of
/// the current one, and `?pass=` is a pass from the viewer-pass route, which
/// the host page carries into this frame. With `?theme=dark`, stamps the dark
/// theme; anything else is light.
pub async fn frame(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<FrameQuery>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    let (artifact, bytes) = match query.version {
        Some(version) => {
            artifact_store::get_at_version(&state.db, &state.data_dir, &artifact_id, version)
                .await
                .map_err(|err| Problem::from_error(&err))?
        }
        None => artifact_store::get(&state.db, &state.data_dir, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?,
    };

    if concealed(&state, &artifact, query.pass.as_deref()).await? {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "artifact {artifact_id} not found"
        ))));
    }

    if artifact.protected {
        return Err(Problem::from_error(&Error::InvalidArgument(format!(
            "artifact {artifact_id} is protected; it unlocks in the host page"
        ))));
    }
    match artifact.kind.as_str() {
        "html" => {}
        "markdown" => {
            return Err(Problem::from_error(&Error::InvalidArgument(
                "markdown renders in the host page".to_string(),
            )));
        }
        other => {
            return Err(Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {artifact_id} kind '{other}' has no frame view"
            ))));
        }
    }

    let theme = match query.theme.as_deref() {
        Some("dark") => "dark",
        _ => "light",
    };
    let content = String::from_utf8_lossy(&bytes);
    let origin = request_origin(&state.config, &headers);
    Ok(frame_response(
        frame_document(&artifact.title, &content, theme),
        &origin,
    ))
}

/// `GET /artifacts/{id}/og.svg`
///
/// Public: a static preview card with the escaped title and description plus
/// the hub wordmark. No external references. With `?version=N`, cards that
/// version instead of the current one. A card names the artifact in a link any
/// reader can follow, so it stays concealed behind a live share like the page
/// itself, and takes no pass: the owner's own reader never asks for one.
pub async fn og_svg(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<VersionQuery>,
) -> std::result::Result<Response, Problem> {
    let (artifact, _bytes) = match query.version {
        Some(version) => {
            artifact_store::get_at_version(&state.db, &state.data_dir, &artifact_id, version)
                .await
                .map_err(|err| Problem::from_error(&err))?
        }
        None => artifact_store::get(&state.db, &state.data_dir, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?,
    };

    if concealed(&state, &artifact, None).await? {
        return Err(Problem::from_error(&Error::NotFound(format!(
            "artifact {artifact_id} not found"
        ))));
    }

    Ok(og_response(og_card(&artifact)))
}

/// A share link record returned by the share management endpoints.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShareResponse {
    pub token: String,
    pub url: String,
    pub version: i64,
    pub created_at: String,
}

/// Request body for creating or rotating a share link.
#[derive(Debug, Default, Deserialize)]
pub struct ShareCreateRequest {
    pub version: Option<i64>,
}

/// `POST /api/v1/artifacts/{id}/share`
///
/// Admin only. Creates or rotates a share link for this artifact.
pub async fn share_create(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> std::result::Result<Json<ShareResponse>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let req: ShareCreateRequest = if body.is_empty() {
        ShareCreateRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|err| {
            Problem::from_error(&Error::InvalidArgument(format!("invalid JSON body: {err}")))
        })?
    };

    let share = artifact_store::create_or_rotate_share(&state.db, &artifact_id, req.version)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    // Relative on purpose: the link is resolved by the browser against the
    // page it is on, so a hub mounted behind a path-stripping proxy gets the
    // prefix right. `request_origin` knows only scheme and authority. H8.
    let url = format!("s/{}", share.token);

    Ok(Json(ShareResponse {
        token: share.token,
        url,
        version: share.version,
        created_at: share.created_at,
    }))
}

/// `DELETE /api/v1/artifacts/{id}/share`
///
/// Admin only. Revokes the active share link for this artifact.
pub async fn share_revoke(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    artifact_store::revoke_share(&state.db, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/artifacts/{id}/share`
///
/// Admin only. Returns the active share link for this artifact if present.
pub async fn share_get(
    State(state): State<AppState>,
    ProblemPath(artifact_id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<ShareResponse>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let share = artifact_store::get_share_for_artifact(&state.db, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    match share {
        Some(s) if s.revoked_at.is_none() => {
            // Relative, like the create route: the browser resolves it against
            // the document, so the path prefix survives.
            let url = format!("s/{}", s.token);
            Ok(Json(ShareResponse {
                token: s.token,
                url,
                version: s.version,
                created_at: s.created_at,
            }))
        }
        _ => Err(Problem::from_error(&Error::NotFound(
            "no active share link".to_string(),
        ))),
    }
}

/// `GET /s/{token}`
///
/// Public: a recipient opens a share link using a revocable token.
/// Serves the pinned version recorded when the share was created.
pub async fn share_host(
    State(state): State<AppState>,
    ProblemPath(token): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    let share = match artifact_store::get_active_share_by_token(&state.db, &token)
        .await
        .map_err(|err| Problem::from_error(&err))?
    {
        Some(s) => s,
        None => {
            return Err(Problem::from_error(&Error::NotFound(
                "artifact not found".to_string(),
            )));
        }
    };

    let (artifact, bytes) = artifact_store::get_at_version(
        &state.db,
        &state.data_dir,
        &share.artifact_id,
        share.version,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    let shown = share.version;
    let pinned = true;
    let origin = request_origin(&state.config, &headers);

    let document = if artifact.protected {
        let ciphertext = String::from_utf8(bytes).map_err(|_| {
            Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {} ciphertext is not UTF-8 text",
                share.artifact_id
            )))
        })?;
        locked_shell(&artifact, &ciphertext, shown, pinned, &origin, Some(&token))
    } else {
        let versions = artifact_store::list_versions(&state.db, &share.artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        let thread = comment_store::list_comments(&state.db, &share.artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        reader_shell(
            &artifact,
            &bytes,
            shown,
            pinned,
            &versions,
            &thread,
            &origin,
            Some(&token),
            None,
        )
    };
    Ok(host_response(document))
}

/// `GET /s/{token}/frame`
///
/// Public: the sandboxed body of a plain HTML artifact for a share link.
/// Serves the pinned version.
pub async fn share_frame(
    State(state): State<AppState>,
    ProblemPath(token): ProblemPath<String>,
    ProblemQuery(query): ProblemQuery<FrameQuery>,
    headers: HeaderMap,
) -> std::result::Result<Response, Problem> {
    let share = match artifact_store::get_active_share_by_token(&state.db, &token)
        .await
        .map_err(|err| Problem::from_error(&err))?
    {
        Some(s) => s,
        None => {
            return Err(Problem::from_error(&Error::NotFound(
                "artifact not found".to_string(),
            )));
        }
    };

    let (artifact, bytes) = artifact_store::get_at_version(
        &state.db,
        &state.data_dir,
        &share.artifact_id,
        share.version,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    if artifact.protected {
        return Err(Problem::from_error(&Error::InvalidArgument(format!(
            "artifact {} is protected; it unlocks in the host page",
            share.artifact_id
        ))));
    }
    match artifact.kind.as_str() {
        "html" => {}
        "markdown" => {
            return Err(Problem::from_error(&Error::InvalidArgument(
                "markdown renders in the host page".to_string(),
            )));
        }
        other => {
            return Err(Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {} kind '{other}' has no frame view",
                share.artifact_id
            ))));
        }
    }

    let theme = match query.theme.as_deref() {
        Some("dark") => "dark",
        _ => "light",
    };
    let content = String::from_utf8_lossy(&bytes);
    let origin = request_origin(&state.config, &headers);
    Ok(frame_response(
        frame_document(&artifact.title, &content, theme),
        &origin,
    ))
}

/// `GET /s/{token}/og.svg`
///
/// Public: a static preview card for a share link.
pub async fn share_og_svg(
    State(state): State<AppState>,
    ProblemPath(token): ProblemPath<String>,
) -> std::result::Result<Response, Problem> {
    let share = match artifact_store::get_active_share_by_token(&state.db, &token)
        .await
        .map_err(|err| Problem::from_error(&err))?
    {
        Some(s) => s,
        None => {
            return Err(Problem::from_error(&Error::NotFound(
                "artifact not found".to_string(),
            )));
        }
    };

    let (artifact, _bytes) = artifact_store::get_at_version(
        &state.db,
        &state.data_dir,
        &share.artifact_id,
        share.version,
    )
    .await
    .map_err(|err| Problem::from_error(&err))?;

    Ok(og_response(og_card(&artifact)))
}

/// The host shell policy: scripts only from the hub origin, no network, the
/// artifact frame only from the hub origin, no inline scripts.
const HOST_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

fn host_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(HOST_CSP),
    );
    // The preview URLs vary with the request host, so a shared cache must
    // not pin one caller's origin and serve it to another. Matches the
    // skill route posture: no-store plus the full origin vary set.
    headers.insert(
        header::VARY,
        HeaderValue::from_static("Host, X-Forwarded-Host, X-Forwarded-Proto"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Serve a frame document with the policy naming the request origin. The
/// origin is built from validated host characters, so it is header-safe.
///
/// No `frame-ancestors` here on purpose: the designed viewer nests this
/// route inside the opaque host frame (the app embeds the host, the host
/// embeds this), and `'self'` never matches an opaque origin. The route
/// serves public HTML only, always sandboxed without same-origin access
/// and without ambient credentials, so any embedder learns nothing and
/// reaches nothing.
fn frame_response(body: String, origin: &str) -> Response {
    let policy = format!(
        "sandbox allow-scripts; default-src 'none'; script-src {origin} 'unsafe-inline'; \
         style-src 'unsafe-inline'; img-src data: blob:; font-src data:; \
         media-src data: blob:; connect-src 'none'; form-action 'none'; \
         base-uri 'none'"
    );
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&policy).expect("frame policy is header-safe"),
    );
    headers.insert(
        header::VARY,
        HeaderValue::from_static("Host, X-Forwarded-Host, X-Forwarded-Proto"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Serve a preview card. Only the content type and nosniff travel with it.
fn og_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Serve raw public bytes as text. Only the content type and nosniff travel
/// with the body.
fn raw_text_response(body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Serve a protected version as JSON for the browser decryptor. Only the
/// content type and nosniff travel with the body.
fn raw_json_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// The reader shell for a plain artifact. The shell carries no author bytes:
/// HTML artifacts load through the frame route, markdown artifacts render in
/// the host page from the inlined source. The version picker appears only
/// when the artifact has more than one version.
#[allow(clippy::too_many_arguments)]
fn reader_shell(
    artifact: &Artifact,
    bytes: &[u8],
    shown: i64,
    pinned: bool,
    versions: &[ArtifactVersion],
    thread: &[Comment],
    origin: &str,
    share_token: Option<&str>,
    pass: Option<&str>,
) -> String {
    let title = escape_html(&artifact.title);
    let with_history = versions.len() > 1 && share_token.is_none();
    let picker = if with_history {
        picker_html(versions, shown)
    } else {
        String::new()
    };
    let frame = if artifact.kind == "html" {
        // Relative to this page's own URL ("/artifacts/{id}" or "/s/{token}"), not the
        // origin root: a leading slash here would collapse to the origin
        // root under a reverse proxy that mounts the hub on a path.
        //
        // The owner pass rides along into the inner frame: it is what read this
        // page, so without it an HTML artifact behind a live share would load
        // a frame the hub conceals.
        let frame_path = match share_token {
            Some(token) => format!("{token}/frame?version={shown}&amp;theme=light"),
            None => format!("{}/frame?version={shown}&amp;theme=light", artifact.id),
        };
        let frame_path = match pass {
            Some(pass) => format!("{frame_path}&amp;pass={}", escape_html(pass)),
            None => frame_path,
        };
        format!(
            "<iframe id=\"hub-frame\" title=\"{title}\" sandbox=\"allow-scripts\" src=\"{frame_path}\"></iframe>\n"
        )
    } else {
        format!("<iframe id=\"hub-frame\" title=\"{title}\" sandbox=\"allow-scripts\"></iframe>\n")
    };
    let meta = script_json(&json!({
        "id": artifact.id,
        "title": artifact.title,
        "kind": artifact.kind,
        "version": shown,
        "protected": false,
        "project_id": artifact.project_id,
        "created_at": artifact.created_at,
        "size_bytes": artifact.size_bytes,
        "actor": artifact.actor,
        "share_token": share_token,
    }));
    let version_blob = if with_history {
        let list: Vec<serde_json::Value> = versions
            .iter()
            .rev()
            .take(50)
            .map(|version| {
                json!({
                    "version": version.version,
                    "label": version.label,
                    "created_at": version.created_at,
                })
            })
            .collect();
        script_json(&json!(list))
    } else {
        "null".to_string()
    };
    // The source, not HTML: the browser already carries a real markdown parser
    // and already uses it for protected artifacts, whose plaintext the server
    // never sees. Sending it the source makes both kinds render the same way,
    // rather than keeping a weaker second renderer alive for the public half.
    let markdown_blob = if artifact.kind == "markdown" {
        script_json(&json!(String::from_utf8_lossy(bytes)))
    } else {
        "null".to_string()
    };
    let thread_html = thread_html(thread, shown);
    let meta_line = format!(
        "<p id=\"hub-meta-line\" class=\"mono\" title=\"{}\">v{shown} · {} · {}</p>\n",
        escape_html(&artifact.created_at),
        escape_html(&artifact.project_id),
        escape_html(&relative_age(&artifact.created_at)),
    );
    let header = header_html(&title, &meta_line, &picker);
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"light\">\n<head>\n{head}\
         <body>\n{header}<main>\n{frame}\
         {thread_html}</main>\n\
         <script type=\"application/json\" id=\"hub-meta\">{meta}</script>\n\
         <script type=\"application/json\" id=\"hub-versions\">{version_blob}</script>\n\
         <script type=\"application/json\" id=\"hub-markdown-body\">{markdown_blob}</script>\n\
         </body>\n</html>\n",
        head = shell_head(artifact, shown, pinned, origin, share_token),
    )
}

/// The locked shell for a protected artifact. It carries the envelope and the
/// ciphertext for the browser decryptor, and nothing else: no picker, no body
/// bytes. The empty frame is filled by the viewer after unlock.
fn locked_shell(
    artifact: &Artifact,
    ciphertext: &str,
    shown: i64,
    pinned: bool,
    origin: &str,
    share_token: Option<&str>,
) -> String {
    let title = escape_html(&artifact.title);
    let meta = script_json(&json!({
        "id": artifact.id,
        "title": artifact.title,
        "kind": artifact.kind,
        "version": shown,
        "protected": true,
        "project_id": artifact.project_id,
        "created_at": artifact.created_at,
        "size_bytes": artifact.size_bytes,
        "actor": artifact.actor,
        "share_token": share_token,
    }));
    let envelope = artifact
        .envelope
        .as_ref()
        .map(script_json)
        .unwrap_or_else(|| "null".to_string());
    let encoded = script_json(&serde_json::Value::String(ciphertext.to_string()));
    let fingerprint = artifact
        .envelope
        .as_ref()
        .and_then(|envelope| envelope.get("salt"))
        .and_then(serde_json::Value::as_str)
        .map(|salt| {
            format!(
                "sha256 {} · {} ciphertext",
                salt_fingerprint(salt),
                format_size(ciphertext.len())
            )
        })
        .unwrap_or_else(|| format!("{} ciphertext", format_size(ciphertext.len())));
    let fingerprint = escape_html(&fingerprint);
    let header = header_html(&title, "", FORGET_CONTROL);
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"light\">\n<head>\n{head}\
         <body>\n{header}<main>\n<p id=\"hub-forget-note\" role=\"status\"></p>\n\
         <div class=\"hub-gate\">\n\
         <div class=\"hub-lock-tile\">{lock}</div>\n\
         <h2>Encrypted artifact</h2>\n\
         <p class=\"hub-gate-copy\">Decrypted on your device. The server stores ciphertext only and never sees the password.</p>\n\
         <form id=\"hub-unlock-form\">\n\
         <label for=\"hub-password\">Password</label>\n\
         <input type=\"text\" name=\"username\" value=\"artifact\" autocomplete=\"username\" hidden>\n\
         <input id=\"hub-password\" name=\"password\" type=\"password\" autocomplete=\"current-password\" placeholder=\"Artifact password\">\n\
         <label class=\"hub-show\"><input id=\"hub-show-password\" type=\"checkbox\"> Show password</label>\n\
         <label class=\"hub-remember\"><input id=\"hub-remember\" type=\"checkbox\" name=\"remember\"> Remember on this device</label>\n\
         <p id=\"hub-unlock-error\" role=\"alert\" hidden></p>\n\
         <button type=\"submit\">Unlock</button>\n</form>\n\
         <p id=\"hub-fingerprint\" class=\"mono\">{fingerprint}</p>\n\
         </div>\n\
         <iframe id=\"hub-frame\" title=\"{title}\" sandbox=\"allow-scripts\" hidden></iframe>\n</main>\n\
         <script type=\"application/json\" id=\"hub-meta\">{meta}</script>\n\
         <script type=\"application/json\" id=\"hub-versions\">null</script>\n\
         <script type=\"application/json\" id=\"hub-markdown-body\">null</script>\n\
         <script type=\"application/json\" id=\"hub-envelope\">{envelope}</script>\n\
         <script type=\"application/json\" id=\"hub-ciphertext\">{encoded}</script>\n\
         </body>\n</html>\n",
        head = shell_head(artifact, shown, pinned, origin, share_token),
        lock = LOCK_SVG,
    )
}

const VIEWER_CSS: &str = include_str!("../../web/artifact-shell.css");

/// The head shared by both shell variants: preview meta tags, the design
/// tokens reused verbatim, a small chrome layer on those tokens, plus the
/// vendor and viewer scripts. No inline scripts.
///
/// The stylesheet and script paths are relative to this page's own URL
/// ("/artifacts/{id}" or "/s/{token}"), one segment below the files it shares with the PWA
/// shell, so "../" reaches them under whatever prefix a proxy mounts the hub
/// on. A leading slash would collapse to the origin root instead.
fn shell_head(
    artifact: &Artifact,
    shown: i64,
    pinned: bool,
    origin: &str,
    share_token: Option<&str>,
) -> String {
    let title = escape_html(&artifact.title);
    let description = escape_html(&artifact.description);
    let pinned_param = match pinned {
        true => format!("?version={shown}"),
        false => String::new(),
    };
    let (og_img, og_url) = match share_token {
        Some(token) => (
            format!("{origin}/s/{token}/og.svg"),
            format!("{origin}/s/{token}"),
        ),
        None => (
            format!(
                "{origin}/artifacts/{id}/og.svg{pinned_param}",
                id = artifact.id
            ),
            format!("{origin}/artifacts/{id}{pinned_param}", id = artifact.id),
        ),
    };
    format!(
        "<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n\
         <meta property=\"og:title\" content=\"{title}\">\n\
         <meta property=\"og:description\" content=\"{description}\">\n\
         <meta property=\"og:image\" content=\"{og_img}\">\n\
         <meta property=\"og:url\" content=\"{og_url}\">\n\
         <meta name=\"twitter:card\" content=\"summary_large_image\">\n\
         <link rel=\"stylesheet\" href=\"../tokens.css\">\n\
         <style>\n{VIEWER_CSS}</style>\n\
         <script src=\"../vendor/marked.js\"></script>\n\
         <script type=\"module\" src=\"../artifact-viewer.mjs\"></script>\n</head>\n",
    )
}

/// Whether the served bytes mention mermaid, in which case the document
/// includes the runtime so fenced diagrams can render.
fn bytes_contains_mermaid(bytes: &[u8]) -> bool {
    bytes
        .windows(b"mermaid".len())
        .any(|window| window == b"mermaid")
}

/// The read-only discussion thread for the reader shell. Only comments at or
/// below the shown version appear; unanchored comments and ones without a
/// stamped version always show. Empty threads render nothing. Every authored
/// string is escaped. The locked shell never calls this: discussion of a
/// protected artifact stays behind auth.
fn thread_html(thread: &[Comment], shown: i64) -> String {
    let visible: Vec<&Comment> = thread
        .iter()
        .filter(|comment| {
            comment
                .anchor_version
                .is_none_or(|version| version <= shown)
        })
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut items = String::new();
    for comment in &visible {
        let author = escape_html(&comment.author);
        let time = escape_html(&relative_age(&comment.created_at));
        let full_time = escape_html(&comment.created_at);
        let body = escape_html(&comment.body);
        let state = if comment.done { "Resolved" } else { "Open" };
        let marker = anchor_marker(comment);
        let id = escape_html(&comment.id);
        items.push_str(&format!(
            "<li data-comment-id=\"{id}\">\n<p class=\"hub-comment-meta\">{author} · <span title=\"{full_time}\">{time}</span> · {state}</p>\n\
             <p class=\"hub-comment-body\">{body}</p>\n{marker}</li>\n"
        ));
    }
    format!(
        "<section id=\"hub-comments\">\n<h2>Comments ({})</h2>\n<ol>\n{items}</ol>\n</section>\n",
        visible.len(),
    )
}

/// The anchor marker line for one comment, empty when unanchored.
fn anchor_marker(comment: &Comment) -> String {
    let Some(anchor) = comment.anchor.as_ref().and_then(Value::as_object) else {
        return String::new();
    };
    let version = comment
        .anchor_version
        .map(|version| format!(" on version {version}"))
        .unwrap_or_default();
    let text = match anchor.get("mode").and_then(Value::as_str) {
        Some("point") => {
            let x = anchor.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = anchor.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            format!("Pinned to point ({x}, {y}){version}")
        }
        Some("text") => {
            let quote = anchor
                .get("quote")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("Quoting {quote:?}{version}")
        }
        _ => format!("Anchored{version}"),
    };
    format!(
        "<p class=\"hub-comment-anchor\">{}</p>\n",
        escape_html(&text)
    )
}

/// Inline line glyphs for the viewer chrome: 1.8px stroke, currentColor.
/// The space after each moveto renders identically and keeps `M` plus
/// digits from reading as something else.
const CHEVRON_SVG: &str = "<svg viewBox=\"0 0 24 24\" width=\"20\" height=\"20\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M 15 5l-7 7 7 7\"/></svg>";
const SUN_SVG: &str = "<svg viewBox=\"0 0 24 24\" width=\"18\" height=\"18\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" aria-hidden=\"true\"><circle cx=\"12\" cy=\"12\" r=\"4\"/><path d=\"M 12 2v2M 12 20v2M 4.9 4.9l1.4 1.4M 17.7 17.7l1.4 1.4M 2 12h2M 20 12h2M 4.9 19.1l1.4-1.4M 17.7 6.3l1.4-1.4\"/></svg>";
const MOON_SVG: &str = "<svg viewBox=\"0 0 24 24\" width=\"18\" height=\"18\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\" hidden><path d=\"M 20 13A8 8 0 1 1 11 4a6.5 6.5 0 0 0 9 9z\"/></svg>";
/// The forget control, in the chrome beside the theme toggle. A remembered
/// password unlocks the artifact without ever showing the gate, so the
/// checkbox that stored it is out of reach by then; the viewer reveals this
/// button in its place, and answers in the live region below the header
/// rather than in a dialog.
const FORGET_CONTROL: &str = "<button id=\"hub-forget\" type=\"button\" aria-label=\"Forget password remembered for this project on this device\" hidden>Forget password</button>\n";

const LOCK_SVG: &str = "<svg viewBox=\"0 0 24 24\" width=\"24\" height=\"24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.8\" stroke-linecap=\"round\" aria-hidden=\"true\"><rect x=\"5\" y=\"11\" width=\"14\" height=\"9\" rx=\"2\"/><path d=\"M 8 11V8a4 4 0 0 1 8 0v3\"/></svg>";

/// The viewer header: back, title with its meta line, then the controls.
/// The viewer shows which theme is active and hides the back button when
/// there is no history to go back to.
fn header_html(title: &str, meta_html: &str, controls: &str) -> String {
    format!(
        "<header>\n<button id=\"hub-back\" type=\"button\" aria-label=\"Back\" hidden>{chevron}</button>\n\
         <div class=\"hub-titleblock\">\n<h1>{title}</h1>\n{meta_html}</div>\n{controls}\
         <button id=\"hub-theme-toggle\" type=\"button\" aria-label=\"Toggle theme\">{sun}{moon}</button>\n</header>\n",
        chevron = CHEVRON_SVG,
        sun = SUN_SVG,
        moon = MOON_SVG,
    )
}

/// Human age of an RFC 3339 timestamp: minutes, hours, days, else the date.
fn relative_age(created_at: &str) -> String {
    let parsed =
        time::OffsetDateTime::parse(created_at, &time::format_description::well_known::Rfc3339);
    let now = time::OffsetDateTime::now_utc();
    match parsed {
        Ok(then) => {
            let seconds = (now - then).whole_seconds().max(0);
            if seconds < 3600 {
                format!("{}m", (seconds.max(1) + 59) / 60)
            } else if seconds < 172800 {
                format!("{}h", seconds / 3600)
            } else if seconds < 2592000 {
                format!("{}d", seconds / 86400)
            } else {
                then.date().to_string()
            }
        }
        Err(_) => created_at.to_string(),
    }
}

/// Short size for the fingerprint line: bytes, kilobytes, or megabytes.
fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        let kb = bytes as f64 / 1024.0;
        if kb.fract() == 0.0 {
            format!("{kb:.0} KB")
        } else {
            format!("{kb:.1} KB")
        }
    } else {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        if mb.fract() == 0.0 {
            format!("{mb:.0} MB")
        } else {
            format!("{mb:.1} MB")
        }
    }
}

/// Short identifier of the key-derivation salt for the fingerprint line:
/// the first and last two bytes as hex. The salt is derivation identity,
/// not a secret.
fn salt_fingerprint(salt_b64: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(salt_b64.as_bytes());
    format!(
        "{:02x}{:02x}…{:02x}{:02x}",
        digest[0], digest[1], digest[30], digest[31]
    )
}

/// The version picker, newest first and capped at 50. Plain artifacts only.
fn picker_html(versions: &[ArtifactVersion], shown: i64) -> String {
    let mut options = String::new();
    for version in versions.iter().rev().take(50) {
        let v_str = format!("v{}", version.version);
        let text = match version
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(label)
                if !label.eq_ignore_ascii_case(&v_str)
                    && !label.eq_ignore_ascii_case(&format!("version {}", version.version)) =>
            {
                format!("{v_str} · {label}")
            }
            _ => v_str,
        };
        let text = escape_html(&text);
        let selected = if version.version == shown {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            "<option value=\"{n}\"{selected}>{text}</option>\n",
            n = version.version,
        ));
    }
    format!(
        "<div id=\"hub-picker-wrap\">\n<label for=\"hub-version-select\">Version</label>\n\
         <select id=\"hub-version-select\">\n{options}</select>\n</div>\n"
    )
}

/// The sandboxed body of a plain HTML artifact. Author bytes travel verbatim;
/// the policy around them names the request origin and never grants
/// same-origin access. When the bytes mention mermaid, the runtime rides
/// along; the shared frame loader always does, so the host can size the
/// frame to its body and diagrams render in the frame theme.
///
/// This page is served at "/artifacts/{id}/frame", two segments below the
/// files it shares with the PWA shell, so "../../" reaches them under
/// whatever prefix a proxy mounts the hub on.
fn frame_document(title: &str, content: &str, theme: &str) -> String {
    let title = escape_html(title);
    let mermaid = if bytes_contains_mermaid(content.as_bytes()) {
        "<script src=\"../../vendor/mermaid.runtime.js\"></script>\n".to_string()
    } else {
        String::new()
    };
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"{theme}\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n\
         <style>html[data-theme=\"light\"]{{color-scheme:light;background:#ffffff;color:#111111}}\
         html[data-theme=\"dark\"]{{color-scheme:dark;background:#111111;color:#eeeeee}}</style>\n\
         {mermaid}<script src=\"../../frame-loader.js\"></script>\n</head>\n<body>\n{content}</body>\n</html>\n"
    )
}

/// The static preview card: 1200 by 630, escaped title and description, the
/// hub wordmark, and no external references.
fn og_card(artifact: &Artifact) -> String {
    let title: String = artifact.title.chars().take(90).collect();
    let description: String = artifact.description.chars().take(160).collect();
    let title = escape_html(&title);
    let description = escape_html(&description);
    let description_text = if description.is_empty() {
        String::new()
    } else {
        format!(
            "<text x=\"96\" y=\"420\" font-family=\"sans-serif\" font-size=\"36\" fill=\"#8b95a5\">{description}</text>\n"
        )
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"630\" viewBox=\"0 0 1200 630\" role=\"img\" aria-label=\"{title}\">\n\
         <title>{title}</title>\n\
         <rect width=\"1200\" height=\"630\" fill=\"#101418\"/>\n\
         <rect x=\"48\" y=\"48\" width=\"1104\" height=\"534\" fill=\"none\" stroke=\"#2a3340\" stroke-width=\"2\"/>\n\
         <text x=\"96\" y=\"140\" font-family=\"sans-serif\" font-size=\"40\" fill=\"#8b95a5\">Agent Hub</text>\n\
         <text x=\"96\" y=\"270\" font-family=\"sans-serif\" font-size=\"72\" fill=\"#ffffff\">{title}</text>\n\
         {description_text}</svg>\n"
    )
}

/// Escape a JSON value so it can sit in a `script` element without closing it.
fn script_json(value: &serde_json::Value) -> String {
    value
        .to_string()
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

#[cfg(test)]
mod tests {
    use super::{format_size, relative_age, salt_fingerprint};

    #[test]
    fn sizes_read_as_bytes_kilobytes_and_megabytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(18), "18 B");
        assert_eq!(format_size(18432), "18 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(2 * 1024 * 1024), "2 MB");
    }

    #[test]
    fn ages_read_as_minutes_hours_days_and_dates() {
        let now = time::OffsetDateTime::now_utc();
        let stamp = |seconds: i64| {
            (now - time::Duration::seconds(seconds))
                .format(&time::format_description::well_known::Rfc3339)
                .expect("format")
        };
        assert_eq!(relative_age(&stamp(30)), "1m");
        assert_eq!(relative_age(&stamp(3000)), "50m");
        assert_eq!(relative_age(&stamp(90000)), "25h");
        assert_eq!(relative_age(&stamp(900000)), "10d");
        assert_eq!(relative_age("2001-02-03T04:05:06Z"), "2001-02-03");
        assert_eq!(relative_age("not a time"), "not a time");
    }

    #[test]
    fn fingerprints_name_the_salt_ends() {
        let fingerprint = salt_fingerprint("c2FsdA==");
        assert_eq!(fingerprint.len(), 4 + 3 + 4);
        assert!(fingerprint.contains('…'));
        assert_eq!(fingerprint, salt_fingerprint("c2FsdA=="));
        assert_ne!(fingerprint, salt_fingerprint("c2FsdB=="));
    }
}
