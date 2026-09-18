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
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::markdown::escape_html;
use crate::store::artifacts::{self as artifact_store, Artifact, ArtifactVersion};
use crate::store::comments::{self as comment_store, AnchorInput, Comment};

/// The artifacts of one project.
#[derive(Debug, Serialize)]
pub struct ArtifactList {
    /// The artifacts, most recently updated first.
    pub artifacts: Vec<Artifact>,
}

/// `GET /api/v1/projects/{id}/artifacts`
///
/// A valid bearer token is required.
pub async fn list(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<ArtifactList>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let artifacts = artifact_store::list(&state.db, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(ArtifactList { artifacts }))
}

/// The content and encryption envelope of one artifact, for the browser
/// decryptor. The server holds no plaintext for a protected artifact.
#[derive(Debug, Serialize)]
pub struct ArtifactContent {
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
    /// The rendered HTML for a public markdown artifact, so the viewer can show
    /// it in a sandboxed frame. Null for every other kind and for a protected
    /// artifact, whose plaintext never reaches the server.
    pub rendered: Option<String>,
    /// The version the content was read at.
    pub version: i64,
    /// Artifact description.
    pub description: String,
    /// Artifact favicon.
    pub favicon: String,
    /// The label of the version read, when set.
    pub label: Option<String>,
}

/// The `?version=N` selector shared by the versioned artifact routes.
#[derive(Debug, Default, Deserialize)]
pub struct VersionQuery {
    /// The version to read. Omitted, the current version is read.
    pub version: Option<i64>,
}

/// The `?version=N&theme=light|dark` selector of the frame route.
#[derive(Debug, Default, Deserialize)]
pub struct FrameQuery {
    /// The version to read. Omitted, the current version is read.
    pub version: Option<i64>,
    /// The frame theme. Only `dark` selects dark; anything else is light.
    pub theme: Option<String>,
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
    Path(artifact_id): Path<String>,
    Query(query): Query<VersionQuery>,
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

    let rendered = (!artifact.protected && artifact.kind == "markdown")
        .then(|| crate::markdown::to_html(&content));

    Ok(Json(ArtifactContent {
        title: artifact.title,
        kind: artifact.kind,
        protected: artifact.protected,
        envelope: artifact.envelope,
        content,
        rendered,
        version: artifact.version,
        description: artifact.description,
        favicon: artifact.favicon,
        label: artifact.label,
    }))
}

/// `GET /api/v1/artifacts/{id}/versions`
///
/// Admin-only. Returns the version history, oldest first.
pub async fn versions(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
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
    Path(artifact_id): Path<String>,
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
    Path(artifact_id): Path<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentPostBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let Json(payload) = body.map_err(|rejection| {
        Problem::from_error(&Error::InvalidArgument(format!(
            "the comment body must be JSON with a body field: {rejection}"
        )))
    })?;
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
    Path((artifact_id, comment_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: std::result::Result<Json<CommentResolveBody>, JsonRejection>,
) -> std::result::Result<Json<Value>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let Json(payload) = body.map_err(|rejection| {
        Problem::from_error(&Error::InvalidArgument(format!(
            "the resolve body must be JSON with a done field: {rejection}"
        )))
    })?;
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
    Path((artifact_id, comment_id)): Path<(String, String)>,
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

/// The public shape of a comment. The delete token hash stays internal, so
/// rows posted over MCP never leak it through the admin listing.
fn comment_view(comment: &Comment) -> Value {
    json!({
        "id": comment.id,
        "artifact_id": comment.artifact_id,
        "author": comment.author,
        "body": comment.body,
        "anchor": comment.anchor.clone().unwrap_or(Value::Null),
        "anchor_version": comment.anchor_version,
        "done": comment.done,
        "created_at": comment.created_at,
    })
}

/// Parse the wire anchor into a validated store input. Unknown modes are
/// rejected; the store checks coordinates, quotes, and sizes.
fn parse_anchor(value: Option<Value>) -> Result<Option<AnchorInput>, Error> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let obj = value.as_object().ok_or_else(|| {
        Error::InvalidArgument("unknown anchor mode, expected point or text".to_string())
    })?;
    match obj.get("mode").and_then(Value::as_str) {
        Some("point") => {
            let x = obj.get("x").and_then(Value::as_f64).ok_or_else(|| {
                Error::InvalidArgument("point anchor needs numeric x and y".to_string())
            })?;
            let y = obj.get("y").and_then(Value::as_f64).ok_or_else(|| {
                Error::InvalidArgument("point anchor needs numeric x and y".to_string())
            })?;
            Ok(Some(AnchorInput::Point { x, y }))
        }
        Some("text") => {
            let quote = obj
                .get("quote")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::InvalidArgument("text anchor needs a quote".to_string()))?;
            Ok(Some(AnchorInput::Text {
                quote: quote.to_string(),
            }))
        }
        _ => Err(Error::InvalidArgument(
            "unknown anchor mode, expected point or text".to_string(),
        )),
    }
}

/// `DELETE /api/v1/artifacts/{id}`
///
/// Admin-only. Deletes the artifact and its history.
pub async fn destroy(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
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
    Path(artifact_id): Path<String>,
    Query(query): Query<VersionQuery>,
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

/// `GET /artifacts/{id}`
///
/// Public: a recipient opens the link without a token. The host shell carries
/// no author bytes: a plain HTML artifact is viewed through the frame route,
/// a plain markdown artifact renders in the host page from the inlined
/// source, and a protected artifact shows the unlock form with the envelope
/// and ciphertext. With `?version=N`, serves that version instead of the
/// current one. All logic lives in the viewer module and the vendor scripts;
/// the shell itself has no inline scripts.
pub async fn host(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    Query(query): Query<VersionQuery>,
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
    let shown = query.version.unwrap_or(artifact.version);
    let pinned = query.version.is_some();
    let origin = request_origin(&state, &headers);

    let document = if artifact.protected {
        // The stored bytes are the client's base64 ciphertext text; they
        // travel verbatim so the browser decryptor decodes exactly once.
        let ciphertext = String::from_utf8(bytes).map_err(|_| {
            Problem::from_error(&Error::InvalidArgument(format!(
                "artifact {artifact_id} ciphertext is not UTF-8 text"
            )))
        })?;
        locked_shell(&artifact, &ciphertext, shown, pinned, &origin)
    } else {
        let versions = artifact_store::list_versions(&state.db, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        let thread = comment_store::list_comments(&state.db, &artifact_id)
            .await
            .map_err(|err| Problem::from_error(&err))?;
        reader_shell(
            &artifact, &bytes, shown, pinned, &versions, &thread, &origin,
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
/// the current one. With `?theme=dark`, stamps the dark theme; anything else
/// is light.
pub async fn frame(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    Query(query): Query<FrameQuery>,
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
    let origin = request_origin(&state, &headers);
    Ok(frame_response(
        frame_document(&artifact.title, &content, theme),
        &origin,
    ))
}

/// `GET /artifacts/{id}/og.svg`
///
/// Public: a static preview card with the escaped title and description plus
/// the hub wordmark. No external references. With `?version=N`, cards that
/// version instead of the current one.
pub async fn og_svg(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    Query(query): Query<VersionQuery>,
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
    Ok(og_response(og_card(&artifact)))
}

/// The host shell policy: scripts only from the hub origin, no network, the
/// artifact frame only from the hub origin, no inline scripts.
const HOST_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; frame-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

/// The hub origin as the caller reached it, for the frame policy and the
/// absolute preview URLs. Mirrors the scheme and host logic of the skill
/// route: the forwarded scheme and host win, then the request host, then the
/// configured bind.
fn request_origin(state: &AppState, headers: &HeaderMap) -> String {
    let scheme = match first_header_value(headers, "x-forwarded-proto").as_deref() {
        Some(value) if value.eq_ignore_ascii_case("https") => "https",
        _ => "http",
    };
    let host = first_header_value(headers, "x-forwarded-host")
        .filter(|value| is_safe_host(value))
        .or_else(|| first_header_value(headers, "host").filter(|value| is_safe_host(value)))
        .unwrap_or_else(|| fallback_authority(state.config.bind));
    format!("{scheme}://{host}")
}

/// The authority to use when no request host is available. An unspecified
/// bind such as `0.0.0.0:8080` is not a usable URL, so it becomes loopback on
/// the same port.
fn fallback_authority(bind: std::net::SocketAddr) -> String {
    if bind.ip().is_unspecified() {
        format!("localhost:{}", bind.port())
    } else {
        bind.to_string()
    }
}

/// The first value of a possibly comma-separated header.
fn first_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Whether a host is safe to echo into a document. The value lands in a URL
/// in served HTML, so it is restricted to the characters a host and optional
/// port can contain.
fn is_safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

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
fn reader_shell(
    artifact: &Artifact,
    bytes: &[u8],
    shown: i64,
    pinned: bool,
    versions: &[ArtifactVersion],
    thread: &[Comment],
    origin: &str,
) -> String {
    let title = escape_html(&artifact.title);
    let with_history = versions.len() > 1;
    let picker = if with_history {
        picker_html(versions, shown)
    } else {
        String::new()
    };
    let frame = if artifact.kind == "html" {
        format!(
            "<iframe id=\"hub-frame\" title=\"{title}\" sandbox=\"allow-scripts\" src=\"/artifacts/{}/frame?version={shown}&amp;theme=light\"></iframe>\n",
            artifact.id,
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
    let markdown_blob = if artifact.kind == "markdown" {
        let source = String::from_utf8_lossy(bytes);
        script_json(&json!(source.as_ref()))
    } else {
        "null".to_string()
    };
    let thread_html = thread_html(thread, shown);
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"light\">\n<head>\n{head}\
          <body>\n<header>\n<h1>{title}</h1>\n{picker}\
          <button id=\"hub-theme-toggle\" type=\"button\">Toggle theme</button>\n</header>\n<main>\n{frame}\
          {thread_html}</main>\n\
         <script type=\"application/json\" id=\"hub-meta\">{meta}</script>\n\
         <script type=\"application/json\" id=\"hub-versions\">{version_blob}</script>\n\
         <script type=\"application/json\" id=\"hub-markdown-body\">{markdown_blob}</script>\n\
         </body>\n</html>\n",
        head = shell_head(artifact, shown, pinned, origin),
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
) -> String {
    let title = escape_html(&artifact.title);
    let meta = script_json(&json!({
        "id": artifact.id,
        "title": artifact.title,
        "kind": artifact.kind,
        "version": shown,
        "protected": true,
    }));
    let envelope = artifact
        .envelope
        .as_ref()
        .map(script_json)
        .unwrap_or_else(|| "null".to_string());
    let encoded = script_json(&serde_json::Value::String(ciphertext.to_string()));
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"light\">\n<head>\n{head}\
         <body>\n<header>\n<h1>{title}</h1>\n\
         <button id=\"hub-theme-toggle\" type=\"button\">Toggle theme</button>\n</header>\n<main>\n\
         <p>This artifact is encrypted. The server does not hold its plaintext. \
         Decryption happens in your browser with the password the sender shared.</p>\n\
         <form id=\"hub-unlock-form\">\n\
         <label for=\"hub-password\">Password</label>\n\
         <input type=\"text\" name=\"username\" value=\"artifact\" autocomplete=\"username\" hidden>\n\
         <input id=\"hub-password\" name=\"password\" type=\"password\" autocomplete=\"current-password\">\n\
         <p id=\"hub-unlock-error\" hidden></p>\n\
         <button type=\"submit\">Unlock</button>\n</form>\n\
         <iframe id=\"hub-frame\" title=\"{title}\" sandbox=\"allow-scripts\"></iframe>\n</main>\n\
         <script type=\"application/json\" id=\"hub-meta\">{meta}</script>\n\
         <script type=\"application/json\" id=\"hub-versions\">null</script>\n\
         <script type=\"application/json\" id=\"hub-markdown-body\">null</script>\n\
         <script type=\"application/json\" id=\"hub-envelope\">{envelope}</script>\n\
         <script type=\"application/json\" id=\"hub-ciphertext\">{encoded}</script>\n\
         </body>\n</html>\n",
        head = shell_head(artifact, shown, pinned, origin),
    )
}

/// The head shared by both shell variants: preview meta tags plus the vendor
/// and viewer scripts. No inline scripts, no stylesheets.
fn shell_head(artifact: &Artifact, shown: i64, pinned: bool, origin: &str) -> String {
    let title = escape_html(&artifact.title);
    let description = escape_html(&artifact.description);
    let pinned = match pinned {
        true => format!("?version={shown}"),
        false => String::new(),
    };
    format!(
        "<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n\
         <meta property=\"og:title\" content=\"{title}\">\n\
         <meta property=\"og:description\" content=\"{description}\">\n\
         <meta property=\"og:image\" content=\"{origin}/artifacts/{id}/og.svg{pinned}\">\n\
         <meta property=\"og:url\" content=\"{origin}/artifacts/{id}{pinned}\">\n\
         <meta name=\"twitter:card\" content=\"summary_large_image\">\n\
         <style>\n\
         :root{{color-scheme:light dark}}\n\
         html,body{{margin:0;padding:0}}\n\
         body{{font-family:system-ui,-apple-system,\"Segoe UI\",sans-serif;line-height:1.5;background:#ffffff;color:#111111}}\n\
         html[data-theme=\"dark\"] body{{background:#141311;color:#ece8e0}}\n\
         body>header{{display:flex;align-items:center;gap:.5rem;flex-wrap:wrap;padding:.5rem .75rem;border-bottom:1px solid #888888}}\n\
         body>header h1{{font-size:1rem;font-weight:600;margin:0;flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}}\n\
         body>header button,body>header select,body>header input{{font:inherit;font-size:.85rem;padding:.4rem .6rem;min-height:44px}}\n\
         button:focus-visible,select:focus-visible,input:focus-visible{{outline:2px solid #4d7cfe;outline-offset:2px}}\n\
         main{{padding:0}}\n\
         main>p,main>form{{margin:1rem;max-width:44rem}}\n\
          iframe#hub-frame{{width:100%;min-height:60vh;border:0;display:block}}\n\
          section#hub-comments{{margin:1rem;max-width:44rem}}\n\
          section#hub-comments ol{{list-style:none;margin:0;padding:0}}\n\
          section#hub-comments li{{border-top:1px solid #888888;padding:.5rem 0}}\n\
          .hub-comment-meta{{font-size:.8rem;margin:0 0 .25rem}}\n\
          .hub-comment-body{{margin:0 0 .25rem;overflow-wrap:anywhere}}\n\
          .hub-comment-anchor{{font-size:.8rem;margin:0}}\n\
         </style>\n\
         <script src=\"/vendor/marked.js\"></script>\n\
         <script type=\"module\" src=\"/artifact-viewer.mjs\"></script>\n</head>\n",
        id = artifact.id,
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
        let time = escape_html(&comment.created_at);
        let body = escape_html(&comment.body);
        let state = if comment.done { "Resolved" } else { "Open" };
        let marker = anchor_marker(comment);
        let id = escape_html(&comment.id);
        items.push_str(&format!(
            "<li data-comment-id=\"{id}\">\n<p class=\"hub-comment-meta\">{author} · {time} · {state}</p>\n\
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

/// The version picker, newest first and capped at 50. Plain artifacts only.
fn picker_html(versions: &[ArtifactVersion], shown: i64) -> String {
    let mut options = String::new();
    for version in versions.iter().rev().take(50) {
        let fallback = format!("Version {}", version.version);
        let label = version.label.as_deref().unwrap_or(&fallback);
        let label = escape_html(label);
        let selected = if version.version == shown {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            "<option value=\"{n}\"{selected}>{label}</option>\n",
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
fn frame_document(title: &str, content: &str, theme: &str) -> String {
    let title = escape_html(title);
    let mermaid = if bytes_contains_mermaid(content.as_bytes()) {
        "<script src=\"/vendor/mermaid.runtime.js\"></script>\n".to_string()
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
         {mermaid}<script src=\"/frame-loader.js\"></script>\n</head>\n<body>\n{content}</body>\n</html>\n"
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
