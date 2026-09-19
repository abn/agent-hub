//! The search route.

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::problem::Problem;
use crate::store::search::{self, SearchQuery, SearchResults};

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

    let results = search::search(&state.db, &params, None)
        .await
        .map_err(|err| Problem::from_error(&err))?;

    Ok(Json(results))
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
        session_id: None,
        limit,
    })
}

fn pairs(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}
