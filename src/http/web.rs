//! The PWA: static assets embedded in the binary and served from the API origin.

use std::sync::LazyLock;

use axum::body::Body;
use axum::http::{HeaderValue, Uri, header};
use axum::response::{IntoResponse, Response};

/// One embedded asset: where it is served, what it holds, and what it is.
struct Asset {
    path: &'static str,
    body: &'static str,
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
        body: include_str!("../../web/index.html"),
        content_type: "text/html; charset=utf-8",
    },
    Asset {
        path: "/app.js",
        body: include_str!("../../web/app.js"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/api.mjs",
        body: include_str!("../../web/api.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/router.mjs",
        body: include_str!("../../web/router.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/dom.mjs",
        body: include_str!("../../web/dom.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/time.mjs",
        body: include_str!("../../web/time.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/prefs.mjs",
        body: include_str!("../../web/prefs.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/keys.mjs",
        body: include_str!("../../web/keys.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/empty.mjs",
        body: include_str!("../../web/empty.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/toast.mjs",
        body: include_str!("../../web/toast.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/dialog.mjs",
        body: include_str!("../../web/dialog.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/composer.mjs",
        body: include_str!("../../web/composer.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/events.mjs",
        body: include_str!("../../web/events.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/projects.mjs",
        body: include_str!("../../web/projects.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/home.mjs",
        body: include_str!("../../web/home.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/inbox.mjs",
        body: include_str!("../../web/inbox.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/feed.mjs",
        body: include_str!("../../web/feed.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/brain-tree.mjs",
        body: include_str!("../../web/brain-tree.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/sessions.mjs",
        body: include_str!("../../web/sessions.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/project.mjs",
        body: include_str!("../../web/project.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/shell.mjs",
        body: include_str!("../../web/shell.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/storage.mjs",
        body: include_str!("../../web/storage.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/search.mjs",
        body: include_str!("../../web/search.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/settings.mjs",
        body: include_str!("../../web/settings.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/project-settings.mjs",
        body: include_str!("../../web/project-settings.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/agents.mjs",
        body: include_str!("../../web/agents.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/artifacts.mjs",
        body: include_str!("../../web/artifacts.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/comments.mjs",
        body: include_str!("../../web/comments.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/connect.mjs",
        body: include_str!("../../web/connect.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/frontmatter.mjs",
        body: include_str!("../../web/frontmatter.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/app.css",
        body: include_str!("../../web/app.css"),
        content_type: "text/css; charset=utf-8",
    },
    Asset {
        path: "/tokens.css",
        body: include_str!("../../web/tokens.css"),
        content_type: "text/css; charset=utf-8",
    },
    Asset {
        path: "/manifest.webmanifest",
        body: include_str!("../../web/manifest.webmanifest"),
        content_type: "application/manifest+json",
    },
    Asset {
        path: "/icon.svg",
        body: include_str!("../../web/icon.svg"),
        content_type: "image/svg+xml",
    },
    Asset {
        path: "/crypto.mjs",
        body: include_str!("../../web/crypto.mjs"),
        content_type: "text/javascript; charset=utf-8",
    },
    Asset {
        path: "/vendor/mermaid.runtime.js",
        body: include_str!("../../web/vendor/mermaid.runtime.js"),
        content_type: "application/javascript; charset=utf-8",
    },
    Asset {
        path: "/artifact-viewer.mjs",
        body: include_str!("../../web/artifact-viewer.mjs"),
        content_type: "application/javascript; charset=utf-8",
    },
    Asset {
        path: "/frame-loader.js",
        body: include_str!("../../web/frame-loader.js"),
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

/// The worker source with its version and cache lists filled in. Stamping
/// once at startup keeps the digest off the request path.
static STAMPED_WORKER: LazyLock<String> = LazyLock::new(|| {
    let required: Vec<&str> = asset_paths()
        .filter(|path| !ON_DEMAND_PATHS.contains(path))
        .collect();
    SERVICE_WORKER
        .replace(VERSION_PLACEHOLDER, &shell_version())
        .replace(ASSETS_PLACEHOLDER, &required.join(","))
        .replace(ON_DEMAND_PLACEHOLDER, &ON_DEMAND_PATHS.join(","))
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
        hasher.update(asset.body.as_bytes());
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
        Some(asset) => respond(asset.body, asset.content_type),
        // Only the table's own paths route here. Anything else gets the same
        // answer the fallback gives every path the hub does not serve.
        None => super::not_found().await.into_response(),
    }
}

/// `GET /sw.js`
///
/// Served with its version stamped in and without HTTP caching: the browser
/// only installs a new worker when these bytes differ, so a cached copy would
/// pin the old shell across an upgrade.
pub async fn service_worker() -> Response {
    let mut response = respond(STAMPED_WORKER.as_str(), "text/javascript; charset=utf-8");
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn respond(body: &'static str, content_type: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
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
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
            ),
        );
    }
    response
}
