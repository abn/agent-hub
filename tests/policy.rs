//! The authorization policy: admin, authenticated agents, confidential projects, and grants.

use agent_hub::error::ErrorCode;
use agent_hub::policy::{Access, Visibility, authorize, visibility};
use agent_hub::principal::Principal;
use agent_hub::store::events::{self, NewEvent};
use agent_hub::store::search::{self, SearchQuery};
use agent_hub::store::{identity, inbox, projects};

mod common;

use common::store::fresh;

fn event(project_id: &str, kind: &str, summary: &str) -> NewEvent {
    NewEvent {
        project_id: project_id.to_string(),
        kind: kind.to_string(),
        summary: summary.to_string(),
        payload: None,
        needs_action: false,
        thread_id: None,
        session_id: None,
    }
}

fn principal(actor: &str, agent_id: &str) -> Principal {
    Principal {
        actor: actor.to_string(),
        agent_id: Some(agent_id.to_string()),
        is_admin: false,
        is_pending: false,
    }
}

fn admin() -> Principal {
    Principal {
        actor: "human".to_string(),
        agent_id: None,
        is_admin: true,
        is_pending: false,
    }
}

async fn forbidden(
    db: &turso::Database,
    who: &Principal,
    project_id: &str,
    access: Access,
) -> bool {
    authorize(db, who, project_id, access)
        .await
        .expect_err("denied")
        .code()
        == ErrorCode::Forbidden
}

#[tokio::test]
async fn the_admin_reaches_everything() {
    let db = fresh("policy-admin").await;
    let agent = identity::create_agent(&db, "agent", "Agent")
        .await
        .expect("create");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared");
    projects::create_with_confidential(&db, "secret", "Secret", true)
        .await
        .expect("secret");

    let admin = admin();
    for (project, access) in [
        ("shared", Access::Read),
        ("shared", Access::Write),
        ("secret", Access::Read),
        ("secret", Access::Write),
        (&agent.personal_project_id, Access::Read),
        (&agent.personal_project_id, Access::Write),
    ] {
        authorize(&db, &admin, project, access)
            .await
            .unwrap_or_else(|err| panic!("admin {access:?} {project}: {err}"));
    }
}

#[tokio::test]
async fn an_authenticated_agent_reads_and_writes_open_projects_and_own_space() {
    let db = fresh("policy-open").await;
    let me = identity::create_agent(&db, "me", "Me")
        .await
        .expect("create me");
    let other = identity::create_agent(&db, "other", "Other")
        .await
        .expect("create other");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared");
    let who = principal("me", "me");

    authorize(&db, &who, "shared", Access::Read)
        .await
        .expect("read shared");
    authorize(&db, &who, "shared", Access::Write)
        .await
        .expect("write shared");
    authorize(&db, &who, &me.personal_project_id, Access::Write)
        .await
        .expect("write own space");
    authorize(&db, &who, &other.personal_project_id, Access::Read)
        .await
        .expect("read another space");
    assert!(
        forbidden(&db, &who, &other.personal_project_id, Access::Write).await,
        "an agent must not write another agent's space"
    );

    let missing = authorize(&db, &who, "ghost", Access::Read)
        .await
        .expect_err("missing project");
    assert_eq!(
        missing.code(),
        ErrorCode::Forbidden,
        "a non-admin cannot tell a missing project from a denied one"
    );
}

#[tokio::test]
async fn a_confidential_project_requires_an_explicit_grant() {
    let db = fresh("policy-confidential").await;
    let _agent = identity::create_agent(&db, "agent", "Agent")
        .await
        .expect("create agent");
    projects::create_with_confidential(&db, "secret", "Secret", true)
        .await
        .expect("secret");
    let who = principal("agent", "agent");

    assert!(forbidden(&db, &who, "secret", Access::Read).await);
    assert!(forbidden(&db, &who, "secret", Access::Write).await);

    identity::add_grant(&db, "agent", "secret")
        .await
        .expect("add grant");
    authorize(&db, &who, "secret", Access::Read)
        .await
        .expect("granted read");
    authorize(&db, &who, "secret", Access::Write)
        .await
        .expect("granted write");

    identity::remove_grant(&db, "agent", "secret")
        .await
        .expect("revoke grant");
    assert!(forbidden(&db, &who, "secret", Access::Read).await);
    assert!(forbidden(&db, &who, "secret", Access::Write).await);
}

#[tokio::test]
async fn search_and_inbox_respect_the_confined_set() {
    let db = fresh("policy-scoping").await;
    projects::create(&db, "p1", "One").await.expect("p1");
    projects::create(&db, "p2", "Two").await.expect("p2");
    events::append(&db, "agent", None, event("p1", "signal", "alpha one"))
        .await
        .expect("append p1");
    events::append(&db, "agent", None, event("p2", "signal", "alpha two"))
        .await
        .expect("append p2");
    events::append(&db, "agent", None, event("p1", "finished", "done p1"))
        .await
        .expect("finished p1");
    events::append(&db, "agent", None, event("p2", "finished", "done p2"))
        .await
        .expect("finished p2");

    let query = SearchQuery {
        text: "alpha".to_string(),
        project_id: None,
        kind: None,
        session_id: None,
        limit: 10,
    };
    let confined = vec!["p1".to_string()];
    let scoped = search::query_visible(&db, &query, Some(&confined))
        .await
        .expect("scoped search");
    assert!(!scoped.is_empty());
    assert!(
        scoped.iter().all(|hit| hit.project_id == "p1"),
        "a confined search returns only its projects"
    );
    let all = search::query(&db, &query).await.expect("unscoped search");
    assert!(
        all.iter().any(|hit| hit.project_id == "p2"),
        "the unscoped search sees every project"
    );

    let items = inbox::list_visible(&db, None, None, 50, Some(&confined))
        .await
        .expect("scoped inbox");
    assert!(!items.is_empty());
    assert!(
        items.iter().all(|item| item.project_id == "p1"),
        "a confined inbox returns only its projects"
    );
    let empty = inbox::list_visible(&db, None, None, 50, Some(&[]))
        .await
        .expect("empty confinement");
    assert!(empty.is_empty(), "an empty confinement yields nothing");
}

#[tokio::test]
async fn a_confined_search_is_not_starved_by_higher_ranked_projects() {
    let db = fresh("policy-starvation").await;
    projects::create(&db, "mine", "Mine").await.expect("mine");
    projects::create(&db, "other", "Other")
        .await
        .expect("other");
    for index in 0..25 {
        events::append(
            &db,
            "agent",
            None,
            event(
                "other",
                "signal",
                &format!("needle needle needle needle {index}"),
            ),
        )
        .await
        .expect("high-rank event");
    }
    events::append(
        &db,
        "agent",
        None,
        event("mine", "signal", "needle in my project"),
    )
    .await
    .expect("visible event");

    let query = SearchQuery {
        text: "needle".to_string(),
        project_id: None,
        kind: None,
        session_id: None,
        limit: 1,
    };
    let visible = vec!["mine".to_string()];
    let hits = search::query_visible(&db, &query, Some(&visible))
        .await
        .expect("confined search");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].project_id, "mine",
        "a visible hit is found even when higher-ranked projects fill the cap"
    );
}

#[tokio::test]
async fn a_confined_search_keeps_relevance_order() {
    let db = fresh("policy-relevance").await;
    projects::create(&db, "mine", "Mine").await.expect("mine");
    events::append(
        &db,
        "agent",
        None,
        event("mine", "signal", "needle needle needle needle"),
    )
    .await
    .expect("relevant event");
    events::append(&db, "agent", None, event("mine", "signal", "needle"))
        .await
        .expect("newer but less relevant event");

    let query = SearchQuery {
        text: "needle".to_string(),
        project_id: None,
        kind: None,
        session_id: None,
        limit: 2,
    };
    let visible = vec!["mine".to_string()];
    let hits = search::query_visible(&db, &query, Some(&visible))
        .await
        .expect("confined search");
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter().all(|hit| hit.project_id == "mine"),
        "only the confined project is returned"
    );
    assert_eq!(
        hits[0].title.as_deref(),
        Some("needle needle needle needle"),
        "text relevance still outranks recency under confinement"
    );
}

#[tokio::test]
async fn visibility_lists_the_reachable_projects() {
    assert!(Visibility::All.as_filter().is_none());
    match Visibility::Only(vec!["a".to_string()]).as_filter() {
        Some(ids) => assert_eq!(ids, ["a".to_string()]),
        None => panic!("a confined set must yield a filter"),
    }

    let db = fresh("policy-visibility").await;
    let agent = identity::create_agent(&db, "agent", "Agent")
        .await
        .expect("create");
    projects::create(&db, "shared", "Shared")
        .await
        .expect("shared");
    projects::create_with_confidential(&db, "secret", "Secret", true)
        .await
        .expect("secret");

    assert_eq!(
        visibility(&db, &admin()).await.expect("admin"),
        Visibility::All
    );

    let who = principal("agent", "agent");
    let visible = visibility(&db, &who).await.expect("agent");
    match visible {
        Visibility::Only(mut ids) => {
            ids.sort();
            let mut expected = vec![agent.personal_project_id.clone(), "shared".to_string()];
            expected.sort();
            assert_eq!(ids, expected);
        }
        Visibility::All => panic!("agent should not see confidential project"),
    }

    identity::add_grant(&db, "agent", "secret")
        .await
        .expect("grant");
    let visible = visibility(&db, &who).await.expect("agent");
    match visible {
        Visibility::Only(mut ids) => {
            ids.sort();
            let mut expected = vec![
                agent.personal_project_id.clone(),
                "shared".to_string(),
                "secret".to_string(),
            ];
            expected.sort();
            assert_eq!(ids, expected);
        }
        Visibility::All => panic!("agent with grant should still see Only filtered list"),
    }
}

/// A corpus row is trusted for its own project and nothing else: a row that
/// names an event, an artifact or a session of another project shows nothing
/// of it, whoever asks.
#[tokio::test]
async fn a_hit_shows_nothing_of_a_row_in_another_project() {
    use agent_hub::store::artifacts::{self, NewArtifact};
    use agent_hub::store::search::{SearchDoc, index_doc};
    use agent_hub::store::sessions;

    let dir = common::temp::TempDir::new("policy-search-fields");
    let db = common::store::open(&dir).await;
    for (id, name) in [("open", "Open"), ("secret", "Secret Plans")] {
        projects::create(&db, id, name).await.expect("project");
    }
    let hidden_event = events::append(
        &db,
        "spy-agent",
        None,
        event("secret", "finished", "whispered plan"),
    )
    .await
    .expect("append");
    let hidden_session = sessions::start(&db, "secret", "covert-run", "spy-agent")
        .await
        .expect("start");
    let hidden_artifact = artifacts::publish(
        &db,
        &dir,
        NewArtifact {
            actor: "spy-agent",
            project_id: "secret",
            title: "dossier",
            description: "",
            favicon: "",
            label: None,
            kind: "markdown",
            content: b"quiet",
            envelope: None,
            session_id: None,
        },
        None,
    )
    .await
    .expect("publish");

    let conn = db.connect().expect("connect");
    for (doc_id, kind, ref_id, session_id) in [
        ("stray:feed", "feed", hidden_event.as_str(), None),
        (
            "stray:artifact",
            "artifact",
            hidden_artifact.id.as_str(),
            None,
        ),
        (
            "stray:brain",
            "brain",
            "/fs/plan.md",
            Some(hidden_session.id.as_str()),
        ),
    ] {
        index_doc(
            &conn,
            SearchDoc {
                doc_id,
                project_id: "open",
                kind,
                ref_id,
                session_id,
                title: Some("stray"),
                body: "haystack needle",
                updated_at: "2026-09-18T00:00:00Z",
            },
        )
        .await
        .expect("index");
    }

    let query = SearchQuery {
        text: "needle".to_string(),
        project_id: None,
        kind: None,
        session_id: None,
        limit: 50,
    };
    let visible = vec!["open".to_string()];
    for confinement in [Some(visible.as_slice()), None] {
        let hits = search::query_visible(&db, &query, confinement)
            .await
            .expect("search");
        let strays: Vec<_> = hits
            .iter()
            .filter(|hit| hit.doc_id.starts_with("stray:"))
            .collect();
        assert_eq!(strays.len(), 3, "the three planted rows are found");
        for hit in strays {
            let shown = serde_json::to_string(hit).expect("serialize");
            for secret in [
                "spy-agent",
                "covert-run",
                "Secret Plans",
                "finished",
                "whispered",
            ] {
                assert!(
                    !shown.contains(secret),
                    "{} shows {secret} of another project: {shown}",
                    hit.doc_id
                );
            }
            let fields = serde_json::to_value(hit).expect("serialize");
            assert!(
                fields.get("version").is_none() && fields.get("size_bytes").is_none(),
                "{} shows an artifact of another project: {shown}",
                hit.doc_id
            );
        }
    }
}
