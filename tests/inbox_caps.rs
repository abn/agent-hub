//! The inbox cap configuration: defaults, the disabling valve, and rejection
//! of an invalid value.

use agent_hub::limits::InboxCaps;

#[test]
fn an_absent_value_takes_the_large_default() {
    let caps = InboxCaps::parse(None, None).expect("defaults parse");
    assert_eq!(caps.per_actor, InboxCaps::DEFAULT_PER_ACTOR);
    assert_eq!(caps.per_project, InboxCaps::DEFAULT_PER_PROJECT);
    assert!(caps.enabled());
}

#[test]
fn an_empty_value_takes_the_default() {
    let caps = InboxCaps::parse(Some(""), Some("")).expect("empty takes the default");
    assert_eq!(caps.per_actor, InboxCaps::DEFAULT_PER_ACTOR);
    assert_eq!(caps.per_project, InboxCaps::DEFAULT_PER_PROJECT);
}

#[test]
fn explicit_values_are_read() {
    let caps = InboxCaps::parse(Some("5"), Some("50")).expect("parse");
    assert_eq!(caps.per_actor, 5);
    assert_eq!(caps.per_project, 50);
}

#[test]
fn zero_disables_a_check() {
    let caps = InboxCaps::parse(Some("0"), Some("0")).expect("parse");
    assert!(!caps.enabled(), "both checks off is disabled");
}

#[test]
fn a_negative_value_is_a_config_error() {
    let err = InboxCaps::parse(Some("-1"), None).expect_err("negative is rejected");
    assert!(
        err.to_string().contains("HUB_INBOX_ACTION_PER_AGENT"),
        "the error names the variable: {err}"
    );
}

#[test]
fn a_non_numeric_value_is_a_config_error() {
    let err = InboxCaps::parse(None, Some("lots")).expect_err("non-numeric is rejected");
    assert!(
        err.to_string().contains("HUB_INBOX_ACTION_PER_PROJECT"),
        "the error names the variable: {err}"
    );
}
