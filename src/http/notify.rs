//! Notify targets: the operator's control surface for where notices go.
//!
//! Every route is admin-gated. A target's URL and token are written here and
//! never read back: a read shows the URL's origin and whether a token is set.
//! The `notify_url` target from the configuration is listed and can be tested,
//! but it is changed in the configuration, not here.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::Error;
use crate::http::auth::{admin, bearer_token};
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::notify::{self, CONFIG_ID, Content, EventSet, Preset, PresetInfo, TargetView};
use crate::store::notify_targets::{self, StoredTarget, TargetChanges};

/// The targets and the presets a new one may use.
#[derive(Debug, Serialize)]
pub struct TargetList {
    pub targets: Vec<TargetView>,
    pub presets: Vec<PresetInfo>,
}

/// A new target.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateTarget {
    pub name: String,
    pub preset: String,
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
    /// The preset's default level when absent.
    #[serde(default)]
    pub content: Option<String>,
    /// `ask` and `deadline` when absent.
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// A change to a target. An absent field keeps its value; an empty token
/// clears it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeTarget {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub events: Option<Vec<String>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// What a test send did.
#[derive(Debug, Serialize)]
pub struct TestOutcome {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub target: Option<TargetView>,
}

fn problem(err: Error) -> Problem {
    Problem::from_error(&err)
}

fn invalid(message: impl Into<String>) -> Problem {
    problem(Error::InvalidArgument(message.into()))
}

fn name(value: &str) -> std::result::Result<String, Problem> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > notify::NAME_CHARS_MAX {
        return Err(invalid(format!(
            "a target's name is 1 to {} characters",
            notify::NAME_CHARS_MAX
        )));
    }
    Ok(value.to_string())
}

fn content(value: &str) -> std::result::Result<Content, Problem> {
    Content::parse(value).ok_or_else(|| {
        invalid(format!(
            "'{value}' is not a content level; use none, title or full"
        ))
    })
}

fn token(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// A URL's scheme, host and port, or nothing when it does not parse.
fn origin(url: &str) -> Option<String> {
    url.trim()
        .parse::<url::Url>()
        .ok()
        .map(|url| url.origin().ascii_serialization())
}

fn view(state: &AppState, id: &str) -> Option<TargetView> {
    state
        .notifier
        .targets()
        .into_iter()
        .find(|target| target.id == id)
}

async fn reload(state: &AppState) -> std::result::Result<(), Problem> {
    let stored = notify_targets::list(&state.db).await.map_err(problem)?;
    state.notifier.reload(&stored);
    Ok(())
}

fn read_only() -> Problem {
    problem(Error::Conflict(
        "the config.toml target is set by notify_url in the configuration; change it there"
            .to_string(),
    ))
}

/// `GET /api/v1/notify/targets`
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<TargetList>, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    Ok(Json(TargetList {
        targets: state.notifier.targets(),
        presets: Preset::ALL.into_iter().map(Preset::info).collect(),
    }))
}

/// `POST /api/v1/notify/targets`
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<CreateTarget>, JsonRejection>,
) -> std::result::Result<(StatusCode, Json<TargetView>), Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let request = json_body(payload, "target body must be valid JSON")?;
    let preset = Preset::parse(&request.preset).ok_or_else(|| {
        invalid(format!(
            "'{}' is not a notify preset; use one of {}",
            request.preset,
            Preset::ALL.map(Preset::as_str).join(", ")
        ))
    })?;
    let token = token(request.token.as_deref());
    let url = notify::check_target(preset, &request.url, token.as_deref()).map_err(problem)?;
    let target = StoredTarget {
        id: crate::store::next_id(),
        name: name(&request.name)?,
        preset,
        url: url.to_string(),
        token,
        content: match request.content.as_deref() {
            Some(value) => content(value)?,
            None => preset.info().default_content,
        },
        events: match &request.events {
            Some(names) => EventSet::parse(names).map_err(problem)?,
            None => EventSet::DEFAULT,
        },
        enabled: request.enabled.unwrap_or(true),
        created_at: String::new(),
    };
    notify_targets::insert(&state.db, &target)
        .await
        .map_err(problem)?;
    reload(&state).await?;
    let created = view(&state, &target.id)
        .ok_or_else(|| problem(Error::Engine("the new target did not load".to_string())))?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// `PATCH /api/v1/notify/targets/{id}`
pub async fn change(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    payload: std::result::Result<Json<ChangeTarget>, JsonRejection>,
) -> std::result::Result<Json<TargetView>, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    if id == CONFIG_ID {
        return Err(read_only());
    }
    let request = json_body(payload, "target body must be valid JSON")?;
    let Some(current) = notify_targets::get(&state.db, &id).await.map_err(problem)? else {
        return Err(problem(Error::NotFound(format!("no notify target '{id}'"))));
    };
    let mut token = request.token.as_deref().map(|value| token(Some(value)));
    // A token is bound to the origin it was given for. A URL that moves to
    // another scheme, host or port drops it unless this request names one, so
    // the old token is never sent to the new host.
    if let Some(url) = &request.url
        && token.is_none()
        && current.token.is_some()
        && origin(url) != origin(&current.url)
    {
        token = Some(None);
    }
    let effective_token = match &token {
        Some(value) => value.clone(),
        None => current.token.clone(),
    };
    let url = request.url.as_deref().unwrap_or(&current.url);
    let url =
        notify::check_target(current.preset, url, effective_token.as_deref()).map_err(problem)?;
    let changes = TargetChanges {
        name: request.name.as_deref().map(name).transpose()?,
        url: request.url.is_some().then(|| url.to_string()),
        token,
        content: request.content.as_deref().map(content).transpose()?,
        events: request
            .events
            .as_deref()
            .map(EventSet::parse)
            .transpose()
            .map_err(problem)?,
        enabled: request.enabled,
    };
    notify_targets::update(&state.db, &id, changes)
        .await
        .map_err(problem)?;
    reload(&state).await?;
    view(&state, &id)
        .map(Json)
        .ok_or_else(|| problem(Error::NotFound(format!("no notify target '{id}'"))))
}

/// `DELETE /api/v1/notify/targets/{id}`
pub async fn delete(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    if id == CONFIG_ID {
        return Err(read_only());
    }
    notify_targets::delete(&state.db, &id)
        .await
        .map_err(problem)?;
    reload(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/notify/targets/{id}/test`
///
/// Sends one test notice now and answers with the outcome. A target that
/// refuses or cannot be reached is a delivered `false`, not a failed request:
/// the route did its job.
pub async fn test(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<TestOutcome>, Problem> {
    admin(&state, bearer_token(&headers).as_deref())?;
    let Some(outcome) = state.notifier.test(&id).await else {
        return Err(problem(Error::NotFound(format!("no notify target '{id}'"))));
    };
    Ok(Json(TestOutcome {
        delivered: outcome.is_ok(),
        error: outcome.err(),
        target: view(&state, &id),
    }))
}
