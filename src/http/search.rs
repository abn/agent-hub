//! The search route.

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use serde_json::Value;

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::bearer_token;
use crate::http::filters::Params;
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
) -> std::result::Result<Json<Value>, Problem> {
    crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;

    let (params, filters) = parse(raw.as_deref()).map_err(|err| Problem::from_error(&err))?;

    let results = search::search_human(&state.db, &params)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    if filters.is_empty() {
        return Ok(Json(serde_json::to_value(results).unwrap_or_default()));
    }
    Ok(Json(narrow(results, &filters)))
}

/// The results with the hits the author filters keep, and the counts of the
/// page and of each group to match.
fn narrow(results: SearchResults, filters: &Params) -> Value {
    let mut value = serde_json::to_value(results).unwrap_or_default();
    let mut total = 0;
    if let Some(groups) = value.get_mut("groups").and_then(Value::as_array_mut) {
        for group in groups.iter_mut() {
            if let Some(hits) = group.get_mut("hits").and_then(Value::as_array_mut) {
                hits.retain(|hit| filters.keeps(hit));
                let kept = hits.len();
                total += kept;
                group["count"] = Value::from(kept);
            }
        }
        groups.retain(|group| group["count"].as_u64().unwrap_or(0) > 0);
    }
    value["count"] = Value::from(total);
    value
}

fn parse(raw: Option<&str>) -> std::result::Result<(SearchQuery, Params), Error> {
    let mut query = String::new();
    let mut scope = None;
    let mut project = None;
    let mut kind = None;
    let mut mode = None;
    let mut limit = 50i64;
    let mut filters = Params::default();

    for (key, value) in pairs(raw) {
        match key.as_str() {
            "q" => query = value,
            "scope" => scope = Some(value),
            "project" => project = Some(value),
            "type" => kind = Some(value),
            "mode" => mode = Some(value),
            "limit" => {
                limit = value
                    .parse()
                    .map_err(|_| Error::InvalidArgument("limit must be an integer".to_string()))?
            }
            other => {
                if !filters.take(other, &value)? {
                    return Err(Error::InvalidArgument(format!(
                        "unknown search parameter '{other}'"
                    )));
                }
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

    let session_id = filters.session.clone();
    Ok((
        SearchQuery {
            text: query,
            project_id,
            kind,
            session_id,
            limit,
            mode,
        },
        filters,
    ))
}

fn pairs(raw: Option<&str>) -> Vec<(String, String)> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}
