//! Settings > Phones: pair, confirm, refuse and unpair the operator's phones,
//! and the one route a phone reads about itself.
//!
//! Every route here but `me` is the operator's own and takes the admin token;
//! a phone is refused them before it reaches a handler
//! ([`crate::phones::guard`]), and each handler refuses one again.
//!
//! A write here also has to come from the hub's own web app. On an open
//! loopback hub the admin token is not required, so without this check a page
//! on another site, open in the operator's browser, could pair a phone of its
//! own and confirm it.

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::error::{CredentialReason, Error};
use crate::http::auth::bearer_token;
use crate::http::problem::{Problem, ProblemPath, json_body};
use crate::phones::Pairing;
use crate::store::devices::{self, Device};

/// The contract version a phone reads to tell whether this hub is new enough.
pub const PHONE_API: u32 = 1;

#[derive(Debug, Serialize)]
pub struct DeviceList {
    /// Whether phones may pair on this hub.
    pub enabled: bool,
    /// Whether phones away from home also reach it through Tendril's public
    /// relay, which carries only encrypted traffic.
    pub relay: bool,
    pub devices: Vec<Device>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmBody {
    /// The check string the operator compared with the phone's.
    pub check_string: String,
}

#[derive(Debug, Serialize)]
pub struct Me {
    pub device_id: String,
    pub name: String,
    pub state: devices::DeviceState,
    pub hub: HubInfo,
    pub phone_api: u32,
}

#[derive(Debug, Serialize)]
pub struct HubInfo {
    pub name: String,
    pub version: &'static str,
    /// The web app's address, `HUB_PUBLIC_URL`, when the operator set one
    /// that is not a loopback address. Never the bind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
}

/// `HUB_PUBLIC_URL`, unless it names a loopback host a phone cannot open.
fn web_url(state: &AppState) -> Option<String> {
    let public = state.config.public_url.as_deref()?;
    let url = url::Url::parse(public).ok()?;
    let loopback = match url.host()? {
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
        url::Host::Domain(name) => {
            let name = name.to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
    };
    (!loopback).then(|| public.to_string())
}

/// The operator, from the admin token, and never from a phone, on a host the
/// hub owns.
fn operator(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Problem> {
    let owned = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| crate::http::origin::host_is_owned(&state.config, host));
    if !owned {
        return Err(Problem::from_error(&Error::Credential(
            CredentialReason::HostNotAllowed,
            "phones are paired through the hub's address or HUB_PUBLIC_URL, not another name"
                .to_string(),
        )));
    }
    let principal = crate::http::auth::admin(state, bearer_token(headers).as_deref())?;
    if principal.device().is_some() {
        return Err(Problem::from_error(&Error::Credential(
            CredentialReason::OperatorOnly,
            "phones are paired and unpaired from the web app".to_string(),
        )));
    }
    Ok(())
}

/// Whether a request comes from a page the hub itself served: the browser's
/// `Sec-Fetch-Site` says `same-origin`, or the `Origin` it sent names the host
/// the request was made to (or `HUB_PUBLIC_URL`). A request that carries
/// neither is not taken as same-origin.
pub fn same_origin(state: &AppState, headers: &HeaderMap) -> bool {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    if let Some(site) = header("sec-fetch-site") {
        return site.eq_ignore_ascii_case("same-origin");
    }
    let Some(origin) = header("origin").and_then(|value| url::Url::parse(value).ok()) else {
        return false;
    };
    let Some(host) = origin.host_str() else {
        return false;
    };
    let authority = match origin.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let named = |candidate: &str| candidate.eq_ignore_ascii_case(&authority);
    header("host").is_some_and(named)
        || state
            .config
            .public_url
            .as_deref()
            .and_then(|public| url::Url::parse(public).ok())
            .is_some_and(|public| public.origin() == origin.origin())
}

/// The operator, from the hub's own web app, for a write.
fn operator_write(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), Problem> {
    operator(state, headers)?;
    if !same_origin(state, headers) {
        return Err(Problem::from_error(&Error::Forbidden(
            "phones are paired and unpaired from the hub's own web app".to_string(),
        )));
    }
    Ok(())
}

/// `GET /api/v1/devices`
///
/// A check string is shown for one phone only, the newest pending one the web
/// app asks the operator to compare, and only to the hub's own web app.
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<DeviceList>, Problem> {
    operator(&state, &headers)?;
    let mut devices = devices::list(&state.db)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    let shown = same_origin(&state, &headers)
        .then(|| {
            devices
                .iter()
                .find(|device| device.state == devices::DeviceState::Pending)
                .map(|device| device.id.clone())
        })
        .flatten();
    for device in &mut devices {
        if shown.as_deref() != Some(device.id.as_str()) {
            device.check_string = None;
        }
    }
    Ok(Json(DeviceList {
        enabled: state.phones.enabled(),
        relay: state.phones.relay(),
        devices,
    }))
}

/// `POST /api/v1/devices/pairing`: a fresh code, its signed URI and QR code.
///
/// The first one starts the hub's Tendril host.
pub async fn pairing(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<Pairing>, Problem> {
    operator_write(&state, &headers)?;
    let pairing = state
        .phones
        .issue(&state)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    Ok(Json(pairing))
}

/// `DELETE /api/v1/devices/pairing`: withdraw every live code, as Close does.
pub async fn withdraw(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<axum::http::StatusCode, Problem> {
    operator_write(&state, &headers)?;
    state.phones.withdraw_codes();
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `POST /api/v1/devices/{id}/confirm`
pub async fn confirm(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<ConfirmBody>, JsonRejection>,
) -> std::result::Result<Json<Device>, Problem> {
    operator_write(&state, &headers)?;
    let body = json_body(body, "confirm body must be JSON with check_string")?;
    state
        .phones
        .confirm(&state, &id, body.check_string.trim())
        .await
        .map_err(|err| Problem::from_error(&err))?;
    state.notify();
    found(&state, &id).await
}

/// `POST /api/v1/devices/{id}/refuse`
pub async fn refuse(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<axum::http::StatusCode, Problem> {
    operator_write(&state, &headers)?;
    state
        .phones
        .refuse(&state, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    state.notify();
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/devices/{id}`: unpair a phone.
pub async fn revoke(
    State(state): State<AppState>,
    ProblemPath(id): ProblemPath<String>,
    headers: HeaderMap,
) -> std::result::Result<Json<Device>, Problem> {
    operator_write(&state, &headers)?;
    state
        .phones
        .revoke(&state, &id)
        .await
        .map_err(|err| Problem::from_error(&err))?;
    state.notify();
    found(&state, &id).await
}

/// `GET /api/v1/devices/me`: the phone the request came from, and the hub.
pub async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> std::result::Result<Json<Me>, Problem> {
    let principal = crate::http::auth::admin(&state, bearer_token(&headers).as_deref())?;
    let Some(device_id) = principal.device() else {
        return Err(Problem::from_error(&Error::NotFound(
            "this request did not come from a paired phone".to_string(),
        )));
    };
    let device = devices::get(&state.db, device_id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .ok_or_else(|| {
            Problem::from_error(&Error::Credential(
                CredentialReason::DeviceUnpaired,
                "this phone was unpaired".to_string(),
            ))
        })?;
    Ok(Json(Me {
        device_id: device.id,
        name: device.name,
        state: device.state,
        hub: HubInfo {
            name: state.host.clone(),
            version: env!("CARGO_PKG_VERSION"),
            web_url: web_url(&state),
        },
        phone_api: PHONE_API,
    }))
}

async fn found(state: &AppState, id: &str) -> std::result::Result<Json<Device>, Problem> {
    devices::get(&state.db, id)
        .await
        .map_err(|err| Problem::from_error(&err))?
        .map(Json)
        .ok_or_else(|| {
            Problem::from_error(&Error::NotFound(format!(
                "no phone {id} is paired with this hub"
            )))
        })
}
