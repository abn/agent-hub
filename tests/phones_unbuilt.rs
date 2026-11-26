//! A hub built without the `phones` feature still serves the device routes,
//! and says why it cannot pair a phone.

#![cfg(not(all(feature = "phones", unix)))]

mod common;

use agent_hub::http::router;
use axum::http::StatusCode;
use common::http::{get, json_body, post, problem_body};
use common::state::open;
use tower::ServiceExt;

#[tokio::test]
async fn a_hub_without_phones_lists_none_and_refuses_to_pair() {
    let state = open("phones-unbuilt").await;
    let request = |mut request: axum::http::Request<axum::body::Body>| {
        let headers = request.headers_mut();
        headers.insert("host", "127.0.0.1:8080".parse().expect("host"));
        headers.insert("sec-fetch-site", "same-origin".parse().expect("site"));
        request
    };
    let list = router(state.clone())
        .oneshot(request(get("/api/v1/devices", Some("Bearer token"))))
        .await
        .expect("router");
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(json_body(list).await["enabled"], false);
    let pairing = router(state.clone())
        .oneshot(request(post(
            "/api/v1/devices/pairing",
            Some("Bearer token"),
            None,
        )))
        .await
        .expect("router");
    assert_eq!(pairing.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        problem_body(pairing).await["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("built without phones"))
    );
}
