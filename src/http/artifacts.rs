//! Artifact REST routes: the project listing and the public render shell.
//!
//! Agent-authored content is untrusted. A public artifact's HTML is served as
//! authored but always framed without same-origin access; a markdown artifact
//! is rendered to HTML by [`crate::markdown`] first, so raw HTML embedded in
//! the source is escaped rather than passed through. Every page the hub builds
//! around an artifact escapes the title first. A protected artifact has no
//! plaintext on the server, so its route can only carry the envelope and the
//! ciphertext for the browser to decrypt.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::markdown::escape_html;
use crate::store::artifacts::{self as artifact_store, Artifact, ArtifactVersion};

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
        let body = serde_json::json!({
            "envelope": artifact.envelope,
            "ciphertext": base64_encode(&bytes),
        });
        Ok(raw_json_response(body.to_string()))
    } else {
        Ok(raw_text_response(bytes))
    }
}

/// `GET /artifacts/{id}`
///
/// Public: a recipient opens the link without a token. The artifact is always
/// wrapped in a document the hub controls and framed without same-origin
/// access, so agent-authored content never runs in the hub origin. A protected
/// artifact returns an unlock shell carrying the envelope and ciphertext; the
/// server holds no plaintext to leak. With `?version=N`, serves that version
/// instead of the current one.
pub async fn render(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
    Query(query): Query<VersionQuery>,
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

    let document = if artifact.protected {
        unlock_shell(&artifact, &bytes)
    } else {
        let content = String::from_utf8_lossy(&bytes);
        match artifact.kind.as_str() {
            "html" => framed_document(&artifact.title, &content),
            "markdown" => rendered_document(&artifact.title, &crate::markdown::to_html(&content)),
            _ => plain_document(&artifact.title, &content),
        }
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

/// Encode bytes with the standard base64 alphabet for the raw protected body.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block: u32 = 0;
        for &byte in chunk {
            block = (block << 8) | u32::from(byte);
        }
        block <<= (3 - chunk.len()) * 8;
        for i in 0..4 {
            if i <= chunk.len() {
                let index = ((block >> (18 - 6 * i)) & 0x3f) as usize;
                out.push(ALPHABET[index] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Frame untrusted HTML in a sandboxed document. The frame has no
/// same-origin access and the content policy disables scripts and external
/// loads, so a published page cannot touch the hub origin.
fn framed_document(title: &str, content: &str) -> String {
    let title = escape_html(title);
    let srcdoc = escape_html(content);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n\
<body style=\"margin:0\">\n<iframe title=\"{title}\" sandbox srcdoc=\"{srcdoc}\" \
style=\"position:fixed;inset:0;width:100%;height:100%;border:0\"></iframe>\n</body>\n</html>\n"
    )
}

/// Wrap rendered markdown in a readable document.
///
/// The body is hub-generated HTML: [`crate::markdown`] escapes every source
/// character, so raw HTML in the markdown is text, not markup. The response
/// still carries the restrictive policy and `sandbox` directive of every
/// artifact page, so the document cannot script or load anything external.
fn rendered_document(title: &str, body: &str) -> String {
    let title = escape_html(title);
    // The artifact title names the document; show it as a heading only when
    // the markdown does not carry its own, so it never appears twice.
    let heading = if body.contains("<h") {
        String::new()
    } else {
        format!("<h1>{title}</h1>\n")
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n</head>\n<body>\n\
<main>\n{heading}{body}</main>\n</body>\n</html>\n"
    )
}

/// Wrap content of an unknown kind in a readable document. The source stays
/// text, so the `<pre>` body is escaped rather than interpreted.
fn plain_document(title: &str, content: &str) -> String {
    let title = escape_html(title);
    let body = escape_html(content);
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

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0xff, 0xfe, 0x00, 0x01]), "//4AAQ==");
    }
}
