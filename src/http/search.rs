//! The search route.

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use serde::Serialize;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::search::{self, SearchGroup, SearchQuery};

/// The results of a search, grouped by corpus family.
#[derive(Debug, Serialize)]
pub struct SearchResults {
    /// The groups, best first.
    pub groups: Vec<SearchGroup>,
}

/// `GET /api/v1/search?q=&scope=&project=&type=&limit=`
///
/// A valid bearer token is required. `q` is mandatory; `scope=global` searches
/// every project, otherwise `project` scopes it when present.
pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> std::result::Result<Json<SearchResults>, Problem> {
    state
        .auth
        .require_admin(bearer_token(&headers).as_deref())
        .map_err(|err| Problem::from_error(&err))?;

    let params = parse(raw.as_deref()).map_err(|err| Problem::from_error(&err))?;

    let results = search::query(&state.db, &params)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let groups = search::group(results);

    Ok(Json(SearchResults { groups }))
}

fn parse(raw: Option<&str>) -> std::result::Result<SearchQuery, Error> {
    let mut query = String::new();
    let mut scope = None;
    let mut project = None;
    let mut kind = None;
    let mut limit = 50i64;

    for (key, value) in pairs(raw) {
        match key.as_str() {
            "q" => query = value,
            "scope" => scope = Some(value),
            "project" => project = Some(value),
            "type" => kind = Some(value),
            "limit" => {
                limit = value
                    .parse()
                    .map_err(|_| Error::InvalidArgument("limit must be an integer".to_string()))?
            }
            other => {
                return Err(Error::InvalidArgument(format!(
                    "unknown search parameter '{other}'"
                )));
            }
        }
    }

    if query.trim().is_empty() {
        return Err(Error::InvalidArgument(
            "the q query parameter is required".to_string(),
        ));
    }

    // `scope=global` clears the project filter; `scope=project` requires one.
    let project_id = match scope.as_deref() {
        Some("global") => None,
        Some("project") => Some(project.ok_or_else(|| {
            Error::InvalidArgument("scope=project requires a project".to_string())
        })?),
        Some(other) => {
            return Err(Error::InvalidArgument(format!("unknown scope '{other}'")));
        }
        None => project,
    };

    Ok(SearchQuery {
        text: query,
        project_id,
        kind,
        limit,
    })
}

fn pairs(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
}

fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
