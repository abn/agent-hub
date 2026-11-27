//! Phones pair and connect through Tendril: a phone scans the hub's signed
//! pairing URI, waits for the operator to confirm its check string, then
//! reaches the REST API as HTTP/2 over its Tendril session as the operator,
//! without the operator's own controls, until it is unpaired.

#![cfg(all(feature = "phones", unix))]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_hub::config::{PhoneRelay, Phones};
use agent_hub::http::router;
use agent_hub::phones::Slot;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::http::{get, json_body, post, problem_body, request};
use common::state::{ADMIN_TOKEN, TestState, open, open_with};
use common::temp::TempDir;
use http_body_util::BodyExt;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Value, json};
use tendril_core::crypto::{SigningKeypair, StaticKeypair};
use tendril_core::identity::EndpointIdentity;
use tendril_core::pairing::{PairOutcome, PairingUri};
use tendril_core::roster::{PeerState, Roster};
use tendril_core::session::SessionCloseCode;
use tendril_runtime::providers::hints::{HintDialerConfig, HintDialers};
use tendril_runtime::{
    ConcurrentSession, ConnectOpts, PairPolicyOptions, RosterAccess, SessionDriver, SessionEvent,
    drive_pair_initiator_with_policy,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

const BEARER: &str = "Bearer token";

/// What the phone's reader records when the stream ends with no close frame.
const ENDED: u16 = 0;

/// A hub whose phone host listens on loopback with no relay, so the test
/// reaches no network beyond this machine.
async fn hub(tag: &str) -> (TestState, agent_hub::app::AppState, TempDir) {
    hub_at(tag, None).await
}

/// As [`hub`], with `HUB_PUBLIC_URL` set to `public_url`.
async fn hub_at(
    tag: &str,
    public_url: Option<&str>,
) -> (TestState, agent_hub::app::AppState, TempDir) {
    assert_eq!(ADMIN_TOKEN, "token");
    let test = open_with(tag, |config| {
        config.public_url = public_url.map(str::to_string);
    })
    .await;
    let tendril = TempDir::new(&format!("{tag}-tendril"));
    let mut state = (*test).clone();
    state.phones = Arc::new(Slot::new(Ok(Phones {
        enabled: true,
        listen: "127.0.0.1:0".parse().expect("address"),
        relay: PhoneRelay::Off,
        wake_file: None,
        state_dir: tendril.path().join("tendril"),
    })));
    (test, state, tendril)
}

/// A request as the hub's own web app makes it: to the hub's address, marked
/// same-origin by the browser.
async fn call(
    state: &agent_hub::app::AppState,
    mut request: Request<Body>,
) -> axum::response::Response {
    let headers = request.headers_mut();
    headers
        .entry("sec-fetch-site")
        .or_insert(axum::http::HeaderValue::from_static("same-origin"));
    headers
        .entry(header::HOST)
        .or_insert(axum::http::HeaderValue::from_static("127.0.0.1:8080"));
    raw(state, request).await
}

/// A request exactly as given, with no browser marks added.
async fn raw(state: &agent_hub::app::AppState, request: Request<Body>) -> axum::response::Response {
    router(state.clone())
        .oneshot(request)
        .await
        .expect("router")
}

/// The phone side: its own Tendril identity and roster, as the app holds them.
struct Phone {
    identity: Arc<EndpointIdentity>,
    roster: RosterAccess,
    host_id: Option<String>,
}

fn dialers() -> Arc<HintDialers> {
    Arc::new(HintDialers::new(HintDialerConfig::default()))
}

impl Phone {
    fn new(tag: u8) -> Self {
        Self {
            identity: Arc::new(EndpointIdentity::from_keys(
                StaticKeypair::from_scalar([tag; 32]),
                SigningKeypair::from_seed([tag.wrapping_add(1); 32]),
            )),
            roster: RosterAccess::memory(Roster::new()),
            host_id: None,
        }
    }

    /// Scan the code: the URI pins the hub, and its signed hints are the
    /// routes Tendril dials.
    async fn pair(&mut self, pairing: &Value) -> String {
        self.try_pair(pairing).await.expect("pairs")
    }

    /// As [`Phone::pair`], with a code the hub may refuse.
    async fn try_pair(&mut self, pairing: &Value) -> Result<String, String> {
        let uri = pairing["uri"].as_str().expect("uri");
        let mut opts = PairPolicyOptions::new(
            "Pixel 9",
            "android",
            pairing["code"].as_str().expect("code"),
            dialers(),
        );
        opts.uri = Some(PairingUri::parse(uri).expect("a valid pairing URI"));
        let result =
            drive_pair_initiator_with_policy(Arc::clone(&self.identity), &self.roster, opts)
                .await
                .map_err(|err| err.to_string())?;
        match result.drive.outcome {
            PairOutcome::InitiatorPending {
                host_id,
                pairing_sas,
                host_pin_verified,
                ..
            } => {
                assert!(host_pin_verified, "the URI pins the hub");
                self.host_id = Some(host_id);
                Ok(pairing_sas)
            }
            other => Err(format!("the hub did not take the claim: {other:?}")),
        }
    }

    /// Tendril 0.1 refuses a pending or revoked phone inside the handshake
    /// and sends no close frame: the connect fails, or the session it returns
    /// ends at once with nothing on it.
    async fn refused(&self) {
        if let Ok(session) = self.connect().await {
            let mut client = Client::over(session, Arc::clone(self.roster.roster())).await;
            assert_eq!(client.closed_with().await, ENDED);
        }
    }

    async fn connect(&self) -> Result<Arc<ConcurrentSession>, String> {
        let host = self.host_id.as_deref().expect("paired");
        SessionDriver::connect_with_policy_and_store(
            Arc::clone(&self.identity),
            host,
            &self.roster,
            ConnectOpts::new(dialers()),
        )
        .await
        .map(|connected| Arc::new(connected.session.into_concurrent()))
        .map_err(|err| err.to_string())
    }
}

/// The phone's HTTP/2 client over its session, as the contract spells it.
struct Client {
    send: hyper::client::conn::http2::SendRequest<Body>,
    closed: tokio::sync::watch::Receiver<Option<u16>>,
}

impl Client {
    async fn over(session: Arc<ConcurrentSession>, roster: Arc<Mutex<Roster>>) -> Self {
        let (phone_side, pump_side) = tokio::io::duplex(64 * 1024);
        let (mut from_client, mut to_client) = tokio::io::split(pump_side);
        let (closed_tx, closed) = tokio::sync::watch::channel(None);
        let reader = Arc::clone(&session);
        tokio::spawn(async move {
            loop {
                match reader.next_event_locked_now(&roster).await {
                    Ok(Some(SessionEvent::AppData(data))) => {
                        if to_client.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(SessionEvent::Closed { code, .. })) => {
                        let _ = closed_tx.send(Some(code));
                        break;
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => {
                        let _ = closed_tx.send(Some(ENDED));
                        break;
                    }
                }
            }
            let _ = to_client.shutdown().await;
        });
        let writer = Arc::clone(&session);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(read) = from_client.read(&mut buf).await {
                if read == 0 || writer.send_data(&buf[..read]).await.is_err() {
                    break;
                }
            }
        });
        let (send, connection) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(phone_side))
                .await
                .expect("HTTP/2 handshake over the session");
        tokio::spawn(connection);
        Self { send, closed }
    }

    /// Send a request with extra headers and hand back the response unread.
    async fn send(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> axum::http::Response<hyper::body::Incoming> {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("http://hub{path}"));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        self.send
            .send_request(builder.body(Body::empty()).expect("request"))
            .await
            .unwrap_or_else(|err| panic!("{method} {path}: {err}"))
    }

    /// POST a JSON body and read the answer.
    async fn post_json(&mut self, path: &str, body: &Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(format!("http://hub{path}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        let response = self
            .send
            .send_request(request)
            .await
            .unwrap_or_else(|err| panic!("POST {path}: {err}"));
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn call(&mut self, method: &str, path: &str, auth: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("http://hub{path}"));
        if let Some(auth) = auth {
            builder = builder.header(header::AUTHORIZATION, auth);
        }
        let response = self
            .send
            .send_request(builder.body(Body::empty()).expect("request"))
            .await
            .unwrap_or_else(|err| panic!("{method} {path}: {err}"));
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn closed_with(&mut self) -> u16 {
        tokio::time::timeout(
            Duration::from_secs(10),
            self.closed.wait_for(Option::is_some),
        )
        .await
        .expect("the session closes")
        .expect("watch")
        .expect("a code")
    }
}

async fn issue(state: &agent_hub::app::AppState) -> Value {
    let response = call(state, post("/api/v1/devices/pairing", Some(BEARER), None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await
}

async fn device(state: &agent_hub::app::AppState, id: &str) -> Option<Value> {
    let list = json_body(call(state, get("/api/v1/devices", Some(BEARER))).await).await;
    list["devices"]
        .as_array()
        .expect("devices")
        .iter()
        .find(|d| d["id"] == id)
        .cloned()
}

/// The phone's row, once the hub has recorded it. Polled for up to 30 s: the
/// row is written when the pairing task finishes, which a loaded host delays.
async fn paired(state: &agent_hub::app::AppState, id: &str) -> Value {
    for _ in 0..600 {
        if let Some(found) = device(state, id).await {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the hub never recorded the phone");
}

#[tokio::test]
async fn a_phone_pairs_is_confirmed_reaches_the_api_and_is_unpaired() {
    let (_test, state, _tendril) = hub("phones-life").await;
    let pairing = issue(&state).await;
    assert!(
        pairing["uri"]
            .as_str()
            .expect("uri")
            .starts_with("tendril://pair?")
    );
    assert!(pairing["code"].as_str().expect("code").starts_with("TND-"));
    assert!(pairing["qr"]["size"].as_u64().expect("qr size") >= 21);
    assert!(
        pairing["address"].is_null(),
        "a loopback listener is no address to type on another device: {pairing}"
    );

    let mut phone = Phone::new(0x42);
    let sas = phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    let pending = paired(&state, &id).await;
    assert_eq!(pending["state"], "pending");
    assert_eq!(
        pending["check_string"], sas,
        "both sides show one check string"
    );
    assert_eq!(pending["name"], "Pixel 9");

    // Pending: Tendril refuses the handshake, so no session reaches the hub.
    phone.refused().await;

    let wrong = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": "aaaa bbbb cccc dddd" })),
        ),
    )
    .await;
    assert_eq!(
        wrong.status(),
        StatusCode::CONFLICT,
        "a check string that does not match"
    );

    let confirmed = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": sas })),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    assert_eq!(json_body(confirmed).await["state"], "active");

    let session = phone.connect().await.expect("a confirmed phone connects");
    let mut client = Client::over(session, Arc::clone(phone.roster.roster())).await;
    let (status, me) = client.call("GET", "/api/v1/devices/me", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["device_id"], id);
    assert_eq!(me["state"], "active");
    assert_eq!(me["phone_api"], 1);

    // The operator's view, with no token: the session is the credential.
    let (status, _) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::OK);

    // The operator's own controls are held back, whatever the phone sends.
    for (method, path) in [
        ("POST", "/api/v1/backups"),
        ("GET", "/api/v1/devices"),
        ("POST", "/api/v1/devices/pairing"),
        ("POST", "/api/v1/members"),
    ] {
        let (status, problem) = client.call(method, path, Some(BEARER)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            problem["details"]["reason"], "operator_only",
            "{method} {path}"
        );
    }

    // Unpairing closes the live session with Tendril's revoked code.
    let revoked = call(
        &state,
        request(
            "DELETE",
            &format!("/api/v1/devices/{id}"),
            Some(BEARER),
            None,
        ),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    assert_eq!(client.closed_with().await, SessionCloseCode::REVOKED);
    assert_eq!(device(&state, &id).await.expect("kept")["state"], "revoked");

    // And Tendril refuses its handshake from then on.
    phone.refused().await;
    state.phones.shutdown().await;
}

#[tokio::test]
async fn a_refused_phone_is_forgotten_and_the_routes_are_the_operators() {
    let (_test, state, _tendril) = hub("phones-refuse").await;

    // Without the admin token nothing here answers.
    let anonymous = call(&state, post("/api/v1/devices/pairing", None, None)).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let me = call(&state, get("/api/v1/devices/me", Some(BEARER))).await;
    assert_eq!(
        me.status(),
        StatusCode::NOT_FOUND,
        "the operator's browser is not a phone"
    );

    let pairing = issue(&state).await;
    let mut phone = Phone::new(0x51);
    phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    paired(&state, &id).await;

    let refused = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/refuse"),
            Some(BEARER),
            None,
        ),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::NO_CONTENT);
    assert!(
        device(&state, &id).await.is_none(),
        "a refused phone is forgotten"
    );

    let again = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/refuse"),
            Some(BEARER),
            None,
        ),
    )
    .await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem_body(again).await["code"], "not_found");
    state.phones.shutdown().await;
}

#[tokio::test]
async fn phones_off_refuses_pairing_and_says_why() {
    let test = open("phones-off").await;
    let mut state = (*test).clone();
    state.phones = Arc::new(Slot::new(Ok(Phones {
        enabled: false,
        listen: "127.0.0.1:0".parse().expect("address"),
        relay: PhoneRelay::Off,
        wake_file: None,
        state_dir: test.dir().join("tendril"),
    })));
    let list = json_body(call(&state, get("/api/v1/devices", Some(BEARER))).await).await;
    assert_eq!(list["enabled"], false);
    let response = call(&state, post("/api/v1/devices/pairing", Some(BEARER), None)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !test.dir().join("tendril").exists(),
        "no Tendril state without phones"
    );
}

/// Every route the router serves, under every method it answers, is either
/// allowed to a phone or refused on purpose. A route added without a decision
/// fails here until someone makes one.
#[tokio::test]
async fn every_route_is_a_deliberate_allow_or_refusal_for_a_phone() {
    use agent_hub::phones::guard::{ALLOWED, REFUSED};

    let test = open("phones-routes").await;
    let mut paths = agent_hub::http::route_paths();
    paths.sort_unstable();
    paths.dedup();
    let mut undecided = Vec::new();
    let mut decided = 0;
    for path in paths {
        let concrete: String = path
            .split('/')
            .map(|segment| {
                if segment.starts_with('{') {
                    "x"
                } else {
                    segment
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
            let response = call(&test, request(method, &concrete, None, None)).await;
            if response.status() == StatusCode::METHOD_NOT_ALLOWED {
                continue;
            }
            let asset = method == "GET" && agent_hub::http::web::asset_paths().any(|a| a == path);
            let allowed = ALLOWED.contains(&(method, path));
            let refused = REFUSED.contains(&(method, path));
            if !(asset || allowed || refused) {
                undecided.push(format!("{method} {path}"));
            }
            decided += 1;
        }
    }
    assert!(decided > 100, "the walk reached the router: {decided}");
    assert!(
        undecided.is_empty(),
        "routes with no phone decision: {undecided:#?}"
    );
}

/// A page on another site cannot drive pairing in the operator's browser, and
/// a check string is shown only to the hub's own web app, for one phone.
#[tokio::test]
async fn device_writes_come_from_the_hubs_own_web_app() {
    let (_test, state, _tendril) = hub("phones-origin").await;
    let cross = |origin: Option<&str>, site: Option<&str>| {
        let mut request = post("/api/v1/devices/pairing", Some(BEARER), None);
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert(header::ORIGIN, origin.parse().expect("origin"));
        }
        if let Some(site) = site {
            request
                .headers_mut()
                .insert("sec-fetch-site", site.parse().expect("site"));
        }
        request
    };
    let cross = |origin: Option<&str>, site: Option<&str>| {
        let mut request = cross(origin, site);
        request
            .headers_mut()
            .insert(header::HOST, "127.0.0.1:8080".parse().expect("host"));
        request
    };
    for (origin, site) in [
        (Some("https://elsewhere.example"), Some("cross-site")),
        (Some("https://elsewhere.example"), None),
        (None, Some("same-site")),
        (None, None),
    ] {
        let response = raw(&state, cross(origin, site)).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{origin:?} {site:?}"
        );
    }
    let mut same_host = cross(Some("http://192.168.1.20:8080"), None);
    same_host
        .headers_mut()
        .insert(header::HOST, "192.168.1.20:8080".parse().expect("host"));
    assert_eq!(raw(&state, same_host).await.status(), StatusCode::OK);

    let pairing = issue(&state).await;
    let mut first = Phone::new(0x61);
    first.pair(&pairing).await;
    let first_id = first.identity.device_id();
    paired(&state, &first_id).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let mut second = Phone::new(0x62);
    let sas = second.pair(&issue(&state).await).await;
    let second_id = second.identity.device_id();
    paired(&state, &second_id).await;

    let list = json_body(call(&state, get("/api/v1/devices", Some(BEARER))).await).await;
    let shown: Vec<&Value> = list["devices"]
        .as_array()
        .expect("devices")
        .iter()
        .filter(|d| !d["check_string"].is_null())
        .collect();
    assert_eq!(shown.len(), 1, "one check string at a time: {list}");
    assert_eq!(shown[0]["id"], second_id.as_str());
    assert_eq!(shown[0]["check_string"], sas.as_str());

    let mut unmarked = get("/api/v1/devices", Some(BEARER));
    unmarked
        .headers_mut()
        .insert(header::HOST, "127.0.0.1:8080".parse().expect("host"));
    let elsewhere = json_body(raw(&state, unmarked).await).await;
    assert!(
        elsewhere["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .all(|d| d["check_string"].is_null()),
        "no check string leaves the hub's own app: {elsewhere}"
    );
    state.phones.shutdown().await;
}

/// A confirmed phone is refused the notify targets and anything that mints a
/// credential that would outlive unpairing it.
#[tokio::test]
async fn a_phone_cannot_reach_notify_targets_or_mint_lasting_links() {
    let (_test, state, _tendril) = hub("phones-allow").await;
    let pairing = issue(&state).await;
    let mut phone = Phone::new(0x71);
    let sas = phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    paired(&state, &id).await;
    let confirmed = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": sas })),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let session = phone.connect().await.expect("connects");
    let mut client = Client::over(session, Arc::clone(phone.roster.roster())).await;
    for (method, path) in [
        ("GET", "/api/v1/notify/targets"),
        ("POST", "/api/v1/notify/targets"),
        ("POST", "/api/v1/artifacts/x/share"),
        ("GET", "/api/v1/artifacts/x/viewer-pass"),
    ] {
        let (status, problem) = client.call(method, path, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert_eq!(
            problem["details"]["reason"], "operator_only",
            "{method} {path}"
        );
    }
    let (status, _) = client.call("GET", "/api/v1/storage", None).await;
    assert_eq!(status, StatusCode::OK, "what the app needs is reached");
    state.phones.shutdown().await;
}

/// A name rebound to the hub's address cannot pair, confirm or unpair a phone:
/// the browser sends that name as both `Host` and `Origin`, so only a host the
/// hub owns is taken. An address, a loopback name and `HUB_PUBLIC_URL` are.
#[tokio::test]
async fn device_routes_refuse_a_host_the_hub_does_not_own() {
    let (_test, state, _tendril) =
        hub_at("phones-rebind", Some("https://hub.example.ts.net")).await;
    let from = |method: &str, path: &str, host: &str, body: Option<Value>| {
        let mut request = request(method, path, Some(BEARER), body);
        let headers = request.headers_mut();
        headers.insert(header::HOST, host.parse().expect("host"));
        headers.insert(
            header::ORIGIN,
            format!("http://{host}").parse().expect("origin"),
        );
        headers.insert("sec-fetch-site", "same-origin".parse().expect("site"));
        request
    };

    let pairing = issue(&state).await;
    let mut phone = Phone::new(0x81);
    let sas = phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    paired(&state, &id).await;

    let confirm = format!("/api/v1/devices/{id}/confirm");
    let unpair = format!("/api/v1/devices/{id}");
    for host in ["evil.example", "evil.example:8080", "hub.lan"] {
        for (method, path, body) in [
            ("POST", "/api/v1/devices/pairing", None),
            (
                "POST",
                confirm.as_str(),
                Some(json!({ "check_string": sas })),
            ),
            ("DELETE", unpair.as_str(), None),
            ("GET", "/api/v1/devices", None),
        ] {
            let response = raw(&state, from(method, path, host, body)).await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{method} {path} on {host}"
            );
            assert_eq!(
                problem_body(response).await["details"]["reason"],
                "host_not_allowed",
                "{method} {path} on {host}"
            );
        }
    }
    assert_eq!(
        device(&state, &id).await.expect("still there")["state"],
        "pending",
        "nothing a rebound name sent took effect"
    );

    // Tendril holds at most four live codes, so most owned hosts are checked
    // on the list, which the same host rule guards.
    for (host, path, method) in [
        ("127.0.0.1:47318", "/api/v1/devices/pairing", "POST"),
        ("hub.example.ts.net", "/api/v1/devices/pairing", "POST"),
        ("192.168.1.20", "/api/v1/devices", "GET"),
        ("[::1]:8080", "/api/v1/devices", "GET"),
        ("localhost:8080", "/api/v1/devices", "GET"),
        ("hub.localhost", "/api/v1/devices", "GET"),
        ("HUB.example.ts.net:443", "/api/v1/devices", "GET"),
    ] {
        let response = raw(&state, from(method, path, host, None)).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{method} {path} on {host}"
        );
    }

    let confirmed = raw(
        &state,
        from(
            "POST",
            &confirm,
            "hub.example.ts.net",
            Some(json!({ "check_string": sas })),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let unpaired = raw(&state, from("DELETE", &unpair, "localhost", None)).await;
    assert_eq!(unpaired.status(), StatusCode::OK);
    state.phones.shutdown().await;
}

/// Pair, confirm and connect one phone: the state, the phone, its device id
/// and a client over its session.
async fn connected(
    tag: &str,
    key: u8,
) -> (
    TestState,
    agent_hub::app::AppState,
    TempDir,
    Phone,
    String,
    Client,
) {
    let (test, state, tendril) = hub(tag).await;
    let pairing = issue(&state).await;
    let mut phone = Phone::new(key);
    let sas = phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    paired(&state, &id).await;
    let confirmed = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": sas })),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let session = phone.connect().await.expect("a confirmed phone connects");
    let client = Client::over(session, Arc::clone(phone.roster.roster())).await;
    (test, state, tendril, phone, id, client)
}

/// The hub checks its own row on every request, so a phone whose row says
/// unpaired is refused on a session that is still open.
#[tokio::test]
async fn a_request_after_the_row_is_unpaired_is_refused_on_the_open_session() {
    let (_test, state, _tendril, _phone, id, mut client) = connected("phones-row", 0x91).await;
    let (status, _) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::OK);
    agent_hub::store::devices::revoke(&state.db, &id)
        .await
        .expect("unpair the row alone");
    let (status, problem) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(problem["details"]["reason"], "device_unpaired");
    // The phone hears that it was unpaired whatever it asks, before the
    // allow-list says no.
    let (status, problem) = client.call("POST", "/api/v1/backups", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(problem["details"]["reason"], "device_unpaired");
    state.phones.shutdown().await;
}

/// A phone unpaired while the Tendril host was not running is revoked in
/// Tendril's roster when the host starts again.
#[tokio::test]
async fn a_phone_unpaired_while_the_host_was_down_is_revoked_on_start() {
    let (_test, state, tendril, phone, id, _client) = connected("phones-down", 0x92).await;
    state.phones.shutdown().await;
    assert!(!state.phones.is_running());
    let unpaired = call(
        &state,
        request(
            "DELETE",
            &format!("/api/v1/devices/{id}"),
            Some(BEARER),
            None,
        ),
    )
    .await;
    assert_eq!(unpaired.status(), StatusCode::OK);
    let roster_state = || {
        tendril_runtime::StateStore::open_or_create(tendril.path().join("tendril"))
            .expect("store")
            .load_roster()
            .expect("roster")
            .get(&id)
            .map(|entry| entry.state)
    };
    assert_eq!(
        roster_state(),
        Some(PeerState::Active),
        "nothing ran to revoke it"
    );
    issue(&state).await;
    assert_eq!(roster_state(), Some(PeerState::Revoked));
    phone.refused().await;
    state.phones.shutdown().await;
}

/// The event stream reaches the phone over the session and ticks when
/// something changes.
#[tokio::test]
async fn the_event_stream_ticks_over_the_session() {
    let (_test, state, _tendril, _phone, _id, mut client) = connected("phones-sse", 0x93).await;
    let response = client.send("GET", "/api/v1/stream", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let ticker = state.clone();
    let nudge = tokio::spawn(async move {
        for _ in 0..20 {
            ticker.notify();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let mut seen = String::new();
    let ticked = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame.expect("frame").into_data() {
                seen.push_str(&String::from_utf8_lossy(&data));
                if seen.contains("tick") {
                    return true;
                }
            }
        }
        false
    })
    .await;
    nudge.abort();
    assert_eq!(ticked, Ok(true), "the stream said: {seen:?}");
    state.phones.shutdown().await;
}

/// The ticks the stream has had since `ticks` was last drained.
fn drained(ticks: &mut tokio::sync::broadcast::Receiver<agent_hub::app::Tick>) -> usize {
    let mut count = 0;
    while ticks.try_recv().is_ok() {
        count += 1;
    }
    count
}

/// The web app waits for a confirmed phone to pick up its confirmation: the
/// phone's first connection after the confirm sets `checked_in_at` and ticks
/// the stream once, and a later one does not tick again.
#[tokio::test]
async fn a_confirmed_phones_first_check_in_is_recorded_and_ticks_once() {
    let (_test, state, _tendril) = hub("phones-check-in").await;
    let pairing = issue(&state).await;
    let mut phone = Phone::new(0x95);
    let sas = phone.pair(&pairing).await;
    let id = phone.identity.device_id();
    paired(&state, &id).await;
    let confirmed = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": sas })),
        ),
    )
    .await;
    assert_eq!(confirmed.status(), StatusCode::OK);
    let confirmed = json_body(confirmed).await;
    assert!(
        confirmed["checked_in_at"].is_null(),
        "nothing has checked in yet: {confirmed}"
    );
    let mut ticks = state.ticker.subscribe();

    let session = phone.connect().await.expect("a confirmed phone connects");
    let mut client = Client::over(session, Arc::clone(phone.roster.roster())).await;
    let (status, me) = client.call("GET", "/api/v1/devices/me", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let row = device(&state, &id).await.expect("kept");
    assert!(
        row["checked_in_at"].is_string(),
        "the first request after the confirm is a check-in: {row}"
    );
    assert_eq!(drained(&mut ticks), 1, "the check-in ticks the stream once");

    let (status, _) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::OK);
    let again = phone.connect().await.expect("connects again");
    let mut second = Client::over(again, Arc::clone(phone.roster.roster())).await;
    let (status, _) = second.call("GET", "/api/v1/devices/me", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(drained(&mut ticks), 0, "a later request does not tick");
    assert!(device(&state, &id).await.expect("kept")["checked_in_at"].is_string());
    state.phones.shutdown().await;
}

/// A clock that steps back after the confirm does not make every later session
/// look like the first: the phone checks in once and stays checked in.
#[tokio::test]
async fn a_clock_stepped_back_after_the_confirm_still_checks_in_once() {
    use agent_hub::store::devices;
    let test = open("phones-clock").await;
    let id = "dev_00000000000000aa";
    devices::record_pending(&test.db, id, "Pixel 9", "android", "aaaa bbbb")
        .await
        .expect("pending");
    devices::activate(&test.db, id).await.expect("confirmed");
    // The confirm as a clock that has since stepped back would have stamped it.
    test.db
        .connect()
        .expect("connect")
        .execute(
            "UPDATE devices SET confirmed_at = '2999-01-01T00:00:00Z' WHERE id = ?1",
            [id],
        )
        .await
        .expect("move the confirm ahead of the clock");
    assert!(
        devices::touch(&test.db, id).await.expect("touch"),
        "the first session is the check-in"
    );
    assert!(
        !devices::touch(&test.db, id).await.expect("touch"),
        "a later session is not"
    );
    let device = devices::get(&test.db, id)
        .await
        .expect("read")
        .expect("kept");
    assert!(device.checked_in_at.is_some(), "{device:?}");
}

/// A token, a cookie or an agent name a phone sends changes nothing: the
/// request is the operator on that phone, who sees a confidential project the
/// member behind the token cannot.
#[tokio::test]
async fn what_a_phone_sends_as_credentials_is_ignored() {
    let (_test, state, _tendril, _phone, _id, mut client) = connected("phones-headers", 0x94).await;
    let token = common::seed::agent_token(&state.db, "laptop", "Laptop").await;
    agent_hub::store::projects::create(&state.db, "secret-plans", "Secret plans")
        .await
        .expect("project");
    agent_hub::store::projects::update(
        &state.db,
        "secret-plans",
        agent_hub::store::projects::ProjectChanges {
            display_name: None,
            confidential: Some(true),
        },
    )
    .await
    .expect("confidential");
    let bearer = format!("Bearer {token}");
    let response = client
        .send(
            "GET",
            "/api/v1/projects",
            &[
                ("authorization", bearer.as_str()),
                ("cookie", "session=elsewhere"),
                ("x-agent-id", "not/a/name"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let listed: Value = serde_json::from_slice(&bytes).expect("json");
    assert!(
        listed["projects"]
            .as_array()
            .expect("projects")
            .iter()
            .any(|project| project["id"] == "secret-plans"),
        "the operator sees the confidential project: {listed}"
    );
    state.phones.shutdown().await;
}

/// A shared artifact's page is concealed from everyone but a share link and
/// the operator's viewer. A phone cannot mint the viewer's pass, so it reads
/// the frame as the operator it is.
#[tokio::test]
async fn a_phone_reads_the_frame_of_a_shared_artifact() {
    let (test, state, _tendril, _phone, _id, mut client) = connected("phones-frame", 0x95).await;
    agent_hub::store::projects::create(&state.db, "proj", "Project")
        .await
        .expect("project");
    let artifact = agent_hub::store::artifacts::publish(
        &state.db,
        test.dir(),
        agent_hub::store::artifacts::NewArtifact {
            writer: &common::seed::writer("laptop"),
            project_id: "proj",
            title: "Notes",
            kind: "html",
            content: b"<h1>Notes</h1>",
            envelope: None,
            description: "",
            label: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");
    let shared = call(
        &state,
        request(
            "POST",
            &format!("/api/v1/artifacts/{}/share", artifact.id),
            Some(BEARER),
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(shared.status(), StatusCode::OK);
    let elsewhere = raw(
        &state,
        get(&format!("/artifacts/{}/frame", artifact.id), None),
    )
    .await;
    assert_eq!(
        elsewhere.status(),
        StatusCode::NOT_FOUND,
        "a share conceals the page"
    );
    let response = client
        .send("GET", &format!("/artifacts/{}/frame", artifact.id), &[])
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    state.phones.shutdown().await;
}

/// A new code withdraws the one before it, and Close withdraws the code on
/// screen: only a code the operator is looking at can be claimed.
#[tokio::test]
async fn only_the_code_on_screen_can_be_claimed() {
    let (_test, state, _tendril) = hub("phones-codes").await;
    let first = issue(&state).await;
    let second = issue(&state).await;
    assert!(
        Phone::new(0xA1).try_pair(&first).await.is_err(),
        "a replaced code"
    );
    Phone::new(0xA2)
        .try_pair(&second)
        .await
        .expect("the newest code pairs");

    let third = issue(&state).await;
    let closed = call(
        &state,
        request("DELETE", "/api/v1/devices/pairing", Some(BEARER), None),
    )
    .await;
    assert_eq!(closed.status(), StatusCode::NO_CONTENT);
    assert!(
        Phone::new(0xA3).try_pair(&third).await.is_err(),
        "a closed code"
    );
    state.phones.shutdown().await;
}

/// With no phone paired and no code live, the host stops on its own and the
/// listener closes.
#[tokio::test]
async fn an_idle_host_stops() {
    let (_test, state, _tendril) = hub("phones-idle").await;
    issue(&state).await;
    assert!(state.phones.is_running());
    let closed = call(
        &state,
        request("DELETE", "/api/v1/devices/pairing", Some(BEARER), None),
    )
    .await;
    assert_eq!(closed.status(), StatusCode::NO_CONTENT);
    let stopped = tokio::time::timeout(Duration::from_secs(20), async {
        while state.phones.is_running() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    assert!(stopped.is_ok(), "the host still runs with nothing to serve");
}

/// Stopping the hub closes a phone's session with a close frame and waits for
/// it, rather than dropping it under a store being checkpointed.
#[tokio::test]
async fn stopping_closes_phone_sessions_cleanly() {
    let (_test, state, _tendril, _phone, _id, mut client) = connected("phones-stop", 0x96).await;
    let (status, _) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(10), state.phones.shutdown())
        .await
        .expect("the host stops within its drain window");
    assert_eq!(client.closed_with().await, SessionCloseCode::NORMAL);
}

/// The first object in `value` whose `payload.action` is `action`.
fn find_payload<'a>(value: &'a Value, action: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            if map
                .get("payload")
                .and_then(|payload| payload.get("action"))
                .and_then(Value::as_str)
                == Some(action)
            {
                return map.get("payload");
            }
            map.values().find_map(|inner| find_payload(inner, action))
        }
        Value::Array(items) => items.iter().find_map(|inner| find_payload(inner, action)),
        _ => None,
    }
}

/// A machine asks for projects when it enrols, and the ask reaches the phone
/// on the inbox item. A phone approves the machine as it asked: it may grant
/// what was asked, or part of it, and nothing else; it cannot rename the
/// member or share. The web app keeps those.
#[tokio::test]
async fn a_phone_approves_a_machine_only_as_it_asked() {
    let (_test, state, _tendril, _phone, _id, mut client) = connected("phones-enrol", 0x97).await;
    for project in ["secret-plans", "docs", "other-secret"] {
        agent_hub::store::projects::create(&state.db, project, project)
            .await
            .expect("project");
        agent_hub::store::projects::update(
            &state.db,
            project,
            agent_hub::store::projects::ProjectChanges {
                display_name: None,
                confidential: Some(true),
            },
        )
        .await
        .expect("confidential");
    }
    let enrolled = call(
        &state,
        request(
            "POST",
            "/api/v1/enrol",
            None,
            Some(json!({
                "member": "laptop",
                "display_name": "Laptop",
                "why": "to work",
                "projects": ["secret-plans", "docs"],
            })),
        ),
    )
    .await;
    assert_eq!(enrolled.status(), StatusCode::ACCEPTED);

    let (status, inbox) = client.call("GET", "/api/v1/inbox", None).await;
    assert_eq!(status, StatusCode::OK);
    let asked = find_payload(&inbox, "enrol_request").expect("the request is on the inbox");
    assert_eq!(
        asked["projects"],
        json!(["secret-plans", "docs"]),
        "{inbox}"
    );

    for body in [
        json!({ "projects": ["other-secret"] }),
        json!({ "projects": ["secret-plans", "other-secret"] }),
        json!({ "member": "renamed" }),
        json!({ "share": true }),
    ] {
        let (status, problem) = client
            .post_json("/api/v1/enrol/laptop/approve", &body)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(problem["details"]["reason"], "operator_only", "{body}");
    }
    let grants = |member: &str| get(&format!("/api/v1/members/{member}/grants"), Some(BEARER));
    let none = json_body(call(&state, grants("laptop")).await).await;
    assert_eq!(
        none["grants"],
        json!([]),
        "nothing a refused approval sent took effect"
    );

    let (status, member) = client
        .post_json(
            "/api/v1/enrol/laptop/approve",
            &json!({ "projects": ["secret-plans"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{member}");
    assert_eq!(member["state"], "active");
    let granted = json_body(call(&state, grants("laptop")).await).await;
    let ids: Vec<&str> = granted["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .filter_map(|grant| grant["project_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["secret-plans"],
        "exactly what the phone chose of the ask"
    );
    state.phones.shutdown().await;
}

/// The phone learns where the web app is only when the operator named it in
/// `HUB_PUBLIC_URL`, and never a loopback address.
#[tokio::test]
async fn devices_me_names_the_web_app_only_from_the_public_url() {
    for (public_url, expected) in [
        (None, None),
        (
            Some("https://hub.example.ts.net"),
            Some("https://hub.example.ts.net"),
        ),
        (Some("http://localhost:8080"), None),
        (Some("http://127.0.0.1:8080"), None),
    ] {
        let (_test, state, _tendril) = hub_at("phones-web-url", public_url).await;
        let pairing = issue(&state).await;
        let mut phone = Phone::new(0xB1);
        let sas = phone.pair(&pairing).await;
        let id = phone.identity.device_id();
        paired(&state, &id).await;
        let confirm = request(
            "POST",
            &format!("/api/v1/devices/{id}/confirm"),
            Some(BEARER),
            Some(json!({ "check_string": sas })),
        );
        assert_eq!(call(&state, confirm).await.status(), StatusCode::OK);
        let session = phone.connect().await.expect("connects");
        let mut client = Client::over(session, Arc::clone(phone.roster.roster())).await;
        let (status, me) = client.call("GET", "/api/v1/devices/me", None).await;
        assert_eq!(status, StatusCode::OK);
        match expected {
            Some(url) => assert_eq!(me["hub"]["web_url"], url, "{public_url:?}"),
            None => assert!(me["hub"].get("web_url").is_none(), "{public_url:?}: {me}"),
        }
        state.phones.shutdown().await;
    }
}

/// A session left idle past the app's threshold still answers its next
/// request at once.
#[tokio::test]
async fn an_idle_session_answers_its_next_request() {
    let (_test, state, _tendril, _phone, _id, mut client) =
        connected("phones-idle-session", 0x98).await;
    let (status, _) = client.call("GET", "/api/v1/home", None).await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(Duration::from_secs(15)).await;
    let answered = tokio::time::timeout(
        Duration::from_secs(3),
        client.call("GET", "/api/v1/devices/me", None),
    )
    .await;
    assert!(
        matches!(answered, Ok((StatusCode::OK, _))),
        "an idle session did not answer within 3 s"
    );
    state.phones.shutdown().await;
}

/// A code is always the long, three-group kind, even on a hub whose only
/// route is local: behind port forwarding or a proxy a stranger's claim
/// arrives from loopback too, so strength never rests on where a peer seems to
/// be.
#[tokio::test]
async fn every_code_is_the_long_kind() {
    let (_test, state, _tendril) = hub("phones-long-code").await;
    for _ in 0..3 {
        let pairing = issue(&state).await;
        let code = pairing["code"].as_str().expect("code");
        let groups: Vec<&str> = code.split('-').collect();
        assert_eq!(groups.len(), 4, "TND and three groups: {code}");
        assert_eq!(groups[0], "TND");
        assert!(groups[1..].iter().all(|group| group.len() == 4), "{code}");
    }
    state.phones.shutdown().await;
}
