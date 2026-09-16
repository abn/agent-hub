//! Artifact REST routes: the project listing and the public render shell.
//!
//! Agent-authored content is untrusted. A public artifact's HTML is served as
//! authored (the design's public link), but every page the hub builds around
//! it escapes the title and content first. A protected artifact has no
//! plaintext on the server, so its route can only carry the envelope and the
//! ciphertext for the browser to decrypt.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use serde::Serialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::artifacts::{self as artifact_store, Artifact};

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
}

/// `GET /api/v1/artifacts/{id}`
///
/// Admin-only. Returns the content for the in-app viewer and decryptor.
pub async fn content(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<ArtifactContent>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let (artifact, bytes) = artifact_store::get(&state.db, &state.data_dir, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let content = String::from_utf8(bytes).map_err(|_| {
        Problem::from_error(&Error::InvalidArgument(format!(
            "artifact {artifact_id} content is not UTF-8 text"
        )))
    })?;

    Ok(Json(ArtifactContent {
        title: artifact.title,
        kind: artifact.kind,
        protected: artifact.protected,
        envelope: artifact.envelope,
        content,
    }))
}

/// `GET /artifacts/{id}`
///
/// Public: a recipient opens the link without a token. The artifact is always
/// wrapped in a document the hub controls and framed without same-origin
/// access, so agent-authored content never runs in the hub origin. A protected
/// artifact returns an unlock shell carrying the envelope and ciphertext; the
/// server holds no plaintext to leak.
pub async fn render(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
) -> std::result::Result<Response, Problem> {
    let (artifact, bytes) = artifact_store::get(&state.db, &state.data_dir, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    let document = if artifact.protected {
        unlock_shell(&artifact, &bytes)
    } else if artifact.kind == "html" {
        framed_document(&artifact.title, &bytes)
    } else {
        markdown_document(&artifact.title, &bytes)
    };
    Ok(html_response(document))
}

fn html_response(body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // No scripts, no external loads, no navigation, and no same-origin access.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; sandbox",
        ),
    );
    response
}

/// Frame untrusted HTML in a sandboxed document. The frame has no
/// same-origin access and the content policy disables scripts and external
/// loads, so a published page cannot touch the hub origin.
fn framed_document(title: &str, content: &[u8]) -> String {
    let title = escape_html(title);
    let srcdoc = escape_html(&String::from_utf8_lossy(content));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n\
<body style=\"margin:0\">\n<iframe title=\"{title}\" sandbox srcdoc=\"{srcdoc}\" \
style=\"position:fixed;inset:0;width:100%;height:100%;border:0\"></iframe>\n</body>\n</html>\n"
    )
}

/// Wrap raw markdown in a readable document. The source stays text, so the
/// `<pre>` body is escaped rather than interpreted.
fn markdown_document(title: &str, content: &[u8]) -> String {
    let title = escape_html(title);
    let body = escape_html(&String::from_utf8_lossy(content));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n<body>\n\
<main>\n<h1>{title}</h1>\n<pre>{body}</pre>\n</main>\n</body>\n</html>\n"
    )
}

/// The page shown for a protected artifact. The browser decrypts it; the
/// envelope and ciphertext are all a client needs.
fn unlock_shell(artifact: &Artifact, ciphertext: &[u8]) -> String {
    let title = escape_html(&artifact.title);
    let envelope = artifact
        .envelope
        .as_ref()
        .map(script_json)
        .unwrap_or_else(|| "null".to_string());
    let ciphertext = json_byte_string(ciphertext);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n<body>\n\
<main>\n<h1>{title}</h1>\n\
<p>This artifact is encrypted. The server does not hold its plaintext. \
Decryption happens in your browser with the password the sender shared.</p>\n\
<p>The unlock interface arrives with the hub app. Until then the envelope and \
ciphertext below are what a client needs to decrypt it.</p>\n\
<script type=\"application/json\" id=\"artifact-envelope\">{envelope}</script>\n\
<script type=\"application/json\" id=\"artifact-ciphertext\">{ciphertext}</script>\n\
</main>\n</body>\n</html>\n"
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

/// Encode bytes as the body of a JSON string, one code point per byte.
///
/// This carries arbitrary ciphertext without padding: a client recovers the
/// exact bytes from the string's code units. Printable ASCII stays literal so
/// the shell remains inspectable.
fn json_byte_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'<' => out.push_str("\\u003c"),
            b'>' => out.push_str("\\u003e"),
            b'&' => out.push_str("\\u0026"),
            0x20..=0x7e => out.push(byte as char),
            other => {
                out.push_str("\\u00");
                out.push(HEX[(other >> 4) as usize] as char);
                out.push(HEX[(other & 0x0f) as usize] as char);
            }
        }
    }
    out.push('"');
    out
}

fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}
