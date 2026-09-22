//! The PWA: static assets embedded in the binary and served from the API origin.

use std::sync::LazyLock;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::app::AppState;

/// One embedded asset: where it is served, what it holds, and what it is.
struct Asset {
    path: &'static str,
    body: &'static [u8],
    content_type: &'static str,
}

/// Every asset this module serves, in serving order. The router takes one
/// route per entry, the service worker precaches the list, and the cache is
/// named after a digest of it, so the offline shell and its lifetime follow
/// the binary rather than a hand-edited constant. Adding an asset is a row
/// here and nothing else.
static SHELL_ASSETS: &[Asset] = &[
    Asset {
        path: "/",
        body: include_bytes!("../../web/index.html"),
        content_type: "text/html; charset=utf-8",
    },
    Asset {
        path: "/app.js",
        body: include_bytes!("../../web/app.js"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/api.mjs",
        body: include_bytes!("../../web/api.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/router.mjs",
        body: include_bytes!("../../web/router.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/dom.mjs",
        body: include_bytes!("../../web/dom.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/shell-layout.mjs",
        body: include_bytes!("../../web/shell-layout.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/time.mjs",
        body: include_bytes!("../../web/time.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/prefs.mjs",
        body: include_bytes!("../../web/prefs.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/keys.mjs",
        body: include_bytes!("../../web/keys.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/empty.mjs",
        body: include_bytes!("../../web/empty.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/toast.mjs",
        body: include_bytes!("../../web/toast.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/dialog.mjs",
        body: include_bytes!("../../web/dialog.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/composer.mjs",
        body: include_bytes!("../../web/composer.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/events.mjs",
        body: include_bytes!("../../web/events.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/projects.mjs",
        body: include_bytes!("../../web/projects.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/home.mjs",
        body: include_bytes!("../../web/home.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/inbox.mjs",
        body: include_bytes!("../../web/inbox.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/feed.mjs",
        body: include_bytes!("../../web/feed.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/brain-tree.mjs",
        body: include_bytes!("../../web/brain-tree.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/sessions.mjs",
        body: include_bytes!("../../web/sessions.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/project.mjs",
        body: include_bytes!("../../web/project.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/shell.mjs",
        body: include_bytes!("../../web/shell.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/storage.mjs",
        body: include_bytes!("../../web/storage.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/search.mjs",
        body: include_bytes!("../../web/search.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/settings.mjs",
        body: include_bytes!("../../web/settings.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/project-settings.mjs",
        body: include_bytes!("../../web/project-settings.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/agents.mjs",
        body: include_bytes!("../../web/agents.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/artifacts.mjs",
        body: include_bytes!("../../web/artifacts.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/comments.mjs",
        body: include_bytes!("../../web/comments.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/glyphs.mjs",
        body: include_bytes!("../../web/glyphs.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/connect.mjs",
        body: include_bytes!("../../web/connect.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/frontmatter.mjs",
        body: include_bytes!("../../web/frontmatter.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/app.css",
        body: include_bytes!("../../web/app.css"),
        content_type: "text/css; charset=utf-8",
    },
    Asset {
        path: "/tokens.css",
        body: include_bytes!("../../web/tokens.css"),
        content_type: "text/css; charset=utf-8",
    },
    Asset {
        path: "/manifest.webmanifest",
        body: include_bytes!("../../web/manifest.webmanifest"),
        content_type: "application/manifest+json",
    },
    Asset {
        path: "/icon.svg",
        body: include_bytes!("../../web/icon.svg"),
        content_type: "image/svg+xml",
    },
    Asset {
        path: "/icon-192.png",
        body: include_bytes!("../../web/icon-192.png"),
        content_type: "image/png",
    },
    Asset {
        path: "/icon-512.png",
        body: include_bytes!("../../web/icon-512.png"),
        content_type: "image/png",
    },
    Asset {
        path: "/icon-512-maskable.png",
        body: include_bytes!("../../web/icon-512-maskable.png"),
        content_type: "image/png",
    },
    Asset {
        path: "/crypto.mjs",
        body: include_bytes!("../../web/crypto.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/vendor/marked.js",
        body: include_bytes!("../../web/vendor/marked.js"),
        content_type: "application/javascript; charset=utf-8",
    },
    Asset {
        path: "/vendor/mermaid.runtime.js",
        body: include_bytes!("../../web/vendor/mermaid.runtime.js"),
        content_type: "application/javascript; charset=utf-8",
    },
    Asset {
        path: "/artifact-viewer.mjs",
        body: include_bytes!("../../web/artifact-viewer.mjs"),
        content_type: "application/javascript; charset=utf-8",
    },
    Asset {
        path: "/frame-loader.js",
        body: include_bytes!("../../web/frame-loader.js"),
        content_type: "application/javascript; charset=utf-8",
    },
];

/// The assets the worker caches without making its install depend on them.
/// An install is all or nothing, so one failed fetch of the diagram runtime,
/// which dwarfs the rest of the shell and only some pages load, would leave
/// the app with no worker at all. They still count towards the version.
static ON_DEMAND_PATHS: &[&str] = &["/vendor/mermaid.runtime.js"];

const SERVICE_WORKER: &str = include_str!("../../web/sw.js");
const VERSION_PLACEHOLDER: &str = "{{version}}";
const ASSETS_PLACEHOLDER: &str = "{{assets}}";
const ON_DEMAND_PLACEHOLDER: &str = "{{on_demand}}";

/// A served path in the form the worker's own `cache.addAll`/`cache.add`
/// resolve correctly: relative to the worker's script URL, which is the app
/// root under whatever prefix a proxy serves it from. The root path is its
/// own directory, so it maps to `./`; an empty string would instead resolve
/// to the worker script itself.
fn relative_asset_path(path: &str) -> String {
    match path {
        "/" => "./".to_string(),
        _ => path.trim_start_matches('/').to_string(),
    }
}

/// The worker source with its version and cache lists filled in. Stamping
/// once at startup keeps the digest off the request path.
static STAMPED_WORKER: LazyLock<String> = LazyLock::new(|| {
    let required: Vec<String> = asset_paths()
        .filter(|path| !ON_DEMAND_PATHS.contains(path))
        .map(relative_asset_path)
        .collect();
    let on_demand: Vec<String> = ON_DEMAND_PATHS
        .iter()
        .copied()
        .map(relative_asset_path)
        .collect();
    SERVICE_WORKER
        .replace(VERSION_PLACEHOLDER, &shell_version())
        .replace(ASSETS_PLACEHOLDER, &required.join(","))
        .replace(ON_DEMAND_PLACEHOLDER, &on_demand.join(","))
});

/// A digest of what the shell is made of.
///
/// Both the path and the body of every asset go in, so a rename counts as a
/// change too, and so does the worker's own source: the cache name is what
/// makes the new worker drop the old cache, so a release that only changes
/// the worker still has to turn the name over. Half a SHA-256 is plenty to
/// tell two builds apart; this names a cache, it guards nothing.
fn shell_version() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(SERVICE_WORKER.as_bytes());
    hasher.update([0]);
    for asset in SHELL_ASSETS {
        hasher.update(asset.path.as_bytes());
        hasher.update([0]);
        hasher.update(asset.body);
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The paths the router serves, in table order.
pub fn asset_paths() -> impl Iterator<Item = &'static str> {
    SHELL_ASSETS.iter().map(|asset| asset.path)
}

/// `GET` for any embedded asset.
///
/// The router registers one route per table entry and they all land here, so
/// an added asset costs a row in the table rather than a route and a handler.
pub async fn asset(uri: Uri) -> Response {
    match SHELL_ASSETS.iter().find(|asset| asset.path == uri.path()) {
        Some(asset) => respond(Body::from(asset.body), asset.content_type),
        // Only the table's own paths route here. Anything else gets the same
        // answer the fallback gives every path the hub does not serve.
        None => super::not_found().await.into_response(),
    }
}

/// `GET /manifest.webmanifest`
///
/// Served from the table's bytes with the node's name folded in, because a
/// browser keys an installed app on its manifest and shows `name` in the
/// launcher: two hubs installed from one browser are otherwise two icons
/// reading "Agent Hub". Only `name` and `short_name` change, and only when a
/// node is configured, so a hub that sets nothing serves exactly what ships.
///
/// Deliberately no `id`. It resolves against the origin of `start_url`, not
/// the manifest's own URL, so a written-down relative `id` would resolve to
/// the origin root for a hub served at `/` and one served at `/hub/` alike
/// and collide them. Left out, it defaults to the resolved `start_url`, which
/// is relative and so already carries whatever prefix serves the app.
pub async fn manifest(State(state): State<AppState>) -> Response {
    let body = match state.config.node_name.as_deref() {
        Some(node) => named_manifest(node),
        None => String::from_utf8_lossy(manifest_bytes()).into_owned(),
    };
    respond(Body::from(body), "application/manifest+json")
}

/// The shipped manifest with the node folded into the two name fields. A
/// manifest that will not parse is a bug in the embedded asset, not in the
/// request, so the shipped bytes are served unchanged rather than failing the
/// install: a launcher entry with a vaguer name beats no manifest at all.
fn named_manifest(node: &str) -> String {
    let shipped = String::from_utf8_lossy(manifest_bytes()).into_owned();
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&shipped) else {
        return shipped;
    };
    let Some(object) = value.as_object_mut() else {
        return shipped;
    };
    object.insert("name".to_string(), json!(format!("Agent Hub ({node})")));
    object.insert("short_name".to_string(), json!(node));
    serde_json::to_string_pretty(&value).unwrap_or(shipped)
}

fn manifest_bytes() -> &'static [u8] {
    SHELL_ASSETS
        .iter()
        .find(|asset| asset.path == MANIFEST_PATH)
        .expect("the asset table serves the manifest")
        .body
}

/// The one table path the router sends to its own handler rather than to
/// `asset`, so registering both would be a duplicate route.
pub const MANIFEST_PATH: &str = "/manifest.webmanifest";

/// `GET /sw.js`
///
/// Served with its version stamped in and without HTTP caching: the browser
/// only installs a new worker when these bytes differ, so a cached copy would
/// pin the old shell across an upgrade.
pub async fn service_worker() -> Response {
    let mut response = respond(
        Body::from(STAMPED_WORKER.as_str()),
        "text/javascript; charset=utf-8",
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn respond(body: Body, content_type: &'static str) -> Response {
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // Module scripts always fetch with CORS, so the artifact host page
    // cannot load its viewer from the PWA's opaque embed without this.
    // Nothing here needs ambient credentials: the API takes bearer tokens
    // in headers, and no response carries per-user secrets.
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    if content_type.starts_with("text/html") {
        // The app is same-origin with the API and loads only its own assets.
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(&SHELL_CSP).expect("the CSP is valid header text"),
        );
    }
    response
}

/// The shell's own inline script, read out of the embedded markup rather
/// than duplicated as a literal: the CSP hash that allows it to run has to
/// track whatever text is actually served, or an edit to one and not the
/// other silently blocks the shell. It is the only `<script>` tag with no
/// `src`; every other script on the page loads its own asset and is covered
/// by `script-src 'self'`.
fn inline_shell_script() -> &'static str {
    let html = std::str::from_utf8(shell_html()).expect("index.html is utf-8");
    let open = "<script>";
    let start = html.find(open).expect("the shell has an inline script") + open.len();
    let end = html[start..]
        .find("</script>")
        .expect("the inline script is closed")
        + start;
    &html[start..end]
}

fn shell_html() -> &'static [u8] {
    SHELL_ASSETS
        .iter()
        .find(|asset| asset.path == "/")
        .expect("the asset table serves index.html")
        .body
}

/// Standard base64, the only encoding a CSP hash source takes. Written by
/// hand rather than pulling in a crate for one 32-byte digest.
fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The shell's content security policy, with a hash source admitting exactly
/// the inline script the shell carries and nothing else inline.
static SHELL_CSP: LazyLock<String> = LazyLock::new(|| {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(inline_shell_script().as_bytes());
    let hash = base64_standard(&digest);
    format!(
        "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'sha256-{hash}'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    )
});
