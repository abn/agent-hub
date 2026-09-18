//! The external origin setting: what is accepted, how it is normalised, and
//! what fails startup.

use agent_hub::config::Config;

#[test]
fn a_bare_origin_is_kept() {
    assert_eq!(
        Config::parse_public_url("https://hub.example").expect("parse"),
        "https://hub.example"
    );
    assert_eq!(
        Config::parse_public_url("http://hub.example:8080").expect("parse"),
        "http://hub.example:8080"
    );
}

#[test]
fn a_trailing_slash_and_a_default_port_are_normalised() {
    assert_eq!(
        Config::parse_public_url("https://hub.example/").expect("parse"),
        "https://hub.example"
    );
    assert_eq!(
        Config::parse_public_url("https://hub.example:443").expect("parse"),
        "https://hub.example"
    );
}

#[test]
fn a_non_http_scheme_is_rejected() {
    let err = Config::parse_public_url("ftp://hub.example").expect_err("scheme is rejected");
    assert!(
        err.to_string().contains("HUB_PUBLIC_URL"),
        "the error names the variable: {err}"
    );
    assert!(matches!(err, agent_hub::error::Error::Config(_)));
}

#[test]
fn a_value_without_a_host_is_rejected() {
    for value in ["hub.example", "https://", "https:"] {
        let err = Config::parse_public_url(value).expect_err("a host is required");
        assert!(matches!(err, agent_hub::error::Error::Config(_)), "{value}");
    }
}

#[test]
fn a_path_query_or_fragment_is_rejected() {
    for value in [
        "https://hub.example/hub",
        "https://hub.example/?a=1",
        "https://hub.example/#frag",
    ] {
        let err = Config::parse_public_url(value).expect_err("only an origin is accepted");
        assert!(matches!(err, agent_hub::error::Error::Config(_)), "{value}");
    }
}

#[test]
fn credentials_are_rejected() {
    let err =
        Config::parse_public_url("https://user:pass@hub.example").expect_err("no credentials");
    assert!(
        err.to_string().contains("credentials"),
        "the error says why: {err}"
    );
}

#[test]
fn a_host_outside_the_origin_character_set_is_rejected() {
    // The value is written into a content policy and into markup, so it is held
    // to the same characters a request host is.
    for value in [
        "https://hub;frame-src.example",
        "https://*.example",
        "https://hub\"x.example",
        "https://hub'x.example",
    ] {
        assert!(
            matches!(
                Config::parse_public_url(value),
                Err(agent_hub::error::Error::Config(_))
            ),
            "{value} should be refused"
        );
    }
}
