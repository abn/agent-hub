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
        .resolve_bearer(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let artifacts = artifact_store::list(&state.db, &project_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(ArtifactList { artifacts }))
}

/// `GET /artifacts/{id}`
///
/// Public: a recipient opens the link without a token. A public artifact is
/// served as HTML. A protected artifact returns an unlock shell carrying the
/// envelope and ciphertext; the server holds no plaintext to leak.
pub async fn render(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
) -> std::result::Result<Response, Problem> {
    let (artifact, bytes) = artifact_store::get(&state.db, &state.data_dir, &artifact_id)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    if artifact.protected {
        return Ok(html_response(unlock_shell(&artifact, &bytes)));
    }

    match artifact.kind.as_str() {
        "html" => Ok(html_response(bytes)),
        _ => Ok(html_response(markdown_document(&artifact.title, &bytes))),
    }
}

fn html_response(body: impl Into<Body>) -> Response {
    let mut response = Response::new(body.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
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
