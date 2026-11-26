//! The members upgrade: a v27 store opens at v28 with its owners placed, its
//! same-name sessions merged, and its delivery state keyed by participant.

mod common;

use std::path::{Path, PathBuf};

use agent_hub::app::AppState;
use agent_hub::author::ParticipantKey;
use agent_hub::brain::BrainStore;
use agent_hub::store::migrate::{self, FailPoint};
use agent_hub::store::schema::MIGRATIONS;
use agent_hub::store::{notifications, open_engine, schema_version, sessions};

use common::temp::TempDir;

const PROJECT: &str = "homelab";
const SURVIVOR: &str = "s-claude";
const OPENCODE: &str = "s-opencode";
const DESKTOP: &str = "s-desktop";

/// Ids that sort in the order the test names them.
fn id(n: u32) -> String {
    format!("01JA{n:022}")
}

/// A store at `version`, built by applying the migrations up to it.
async fn store_at(dir: &Path, version: i64) -> turso::Database {
    let db = open_engine(&dir.join("hub.db")).await.expect("open engine");
    let conn = db.connect().expect("connect");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL)",
        (),
    )
    .await
    .expect("version table");
    for migration in MIGRATIONS.iter().filter(|m| m.version <= version) {
        conn.execute_batch(migration.ddl).await.expect("apply ddl");
        conn.execute(
            "INSERT INTO schema_version(version) VALUES (?1)",
            [migration.version],
        )
        .await
        .expect("record version");
    }
    db
}

async fn sql(db: &turso::Database, statement: &str) {
    db.connect()
        .expect("connect")
        .execute_batch(statement)
        .await
        .unwrap_or_else(|err| panic!("{statement}: {err}"));
}

async fn texts(db: &turso::Database, query: &str) -> Vec<String> {
    let conn = db.connect().expect("connect");
    let mut rows = conn.query(query, ()).await.expect("query");
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.expect("row") {
        out.push(match row.get_value(0).expect("value") {
            turso::Value::Text(text) => text,
            turso::Value::Integer(n) => n.to_string(),
            turso::Value::Null => "NULL".to_string(),
            other => format!("{other:?}"),
        });
    }
    out
}

async fn agent(db: &turso::Database, member: &str) {
    sql(
        db,
        &format!(
            "INSERT INTO agents(id, display_name, personal_project_id, created_at, state)
             VALUES ('{member}', '{member}', 'space-{}', '2026-01-01T00:00:00Z', 'active')",
            member.replace('/', "-")
        ),
    )
    .await;
}

async fn session(db: &turso::Database, id: &str, name: &str, owner: &str, status: &str, at: &str) {
    sql(
        db,
        &format!(
            "INSERT INTO sessions(id, project_id, session_name, agent, status, brain_path, created_at, last_activity)
             VALUES ('{id}', '{PROJECT}', '{name}', '{owner}', '{status}', 'sessions/{PROJECT}/{id}.db',
                     '2026-01-01T00:00:00Z', '{at}')"
        ),
    )
    .await;
}

async fn event(db: &turso::Database, id: &str, kind: &str, actor: &str, session: Option<&str>) {
    let session = session.map_or("NULL".to_string(), |s| format!("'{s}'"));
    sql(
        db,
        &format!(
            "INSERT INTO events(id, project_id, kind, actor, summary, payload, thread_id, needs_action, created_at, session_id)
             VALUES ('{id}', '{PROJECT}', '{kind}', '{actor}', 'ask', '{{\"body\":\"b\"}}', '{id}', 1,
                     '2026-01-01T00:00:00Z', {session})"
        ),
    )
    .await;
}

/// A question asked in a session and answered, both before the upgrade.
async fn answered(db: &turso::Database, question: &str, answer: &str, actor: &str, session: &str) {
    event(db, question, "question", actor, Some(session)).await;
    sql(
        db,
        &format!(
            "INSERT INTO inbox(event_id, status, assigned_to, updated_at)
             VALUES ('{question}', 'resolved', 'human', '2026-01-01T00:00:00Z');
             INSERT INTO events(id, project_id, kind, actor, summary, payload, thread_id, needs_action, created_at)
             VALUES ('{answer}', '{PROJECT}', 'answer', 'human', 're', '{{\"body\":\"yes\"}}', '{question}', 0,
                     '2026-01-01T00:00:00Z')"
        ),
    )
    .await;
}

async fn put(dir: &Path, session: &str, path: &str, bytes: &[u8]) {
    let brain = BrainStore::for_data_dir(dir)
        .open(PROJECT, session)
        .await
        .expect("open brain");
    brain.put(path, bytes).await.expect("put");
    brain.checkpoint().await.expect("checkpoint");
}

/// The members, project and three same-name sessions the spec's fixture
/// names: `laptop/claude` (active), `laptop/opencode` (ended) and `desktop`
/// (ended), all `feat-x` in `homelab`.
async fn merge_fixture(dir: &Path) -> turso::Database {
    let db = store_at(dir, 27).await;
    for member in ["laptop", "claude-code/laptop", "desktop"] {
        agent(&db, member).await;
    }
    sql(
        &db,
        &format!(
            "INSERT INTO projects(id, display_name, created_at) VALUES ('{PROJECT}', 'Homelab', '2026-01-01T00:00:00Z')"
        ),
    )
    .await;
    session(
        &db,
        SURVIVOR,
        "feat-x",
        "laptop/claude",
        "active",
        "2026-01-03T00:00:00Z",
    )
    .await;
    session(
        &db,
        OPENCODE,
        "feat-x",
        "laptop/opencode",
        "ended",
        "2026-01-02T00:00:00Z",
    )
    .await;
    session(
        &db,
        DESKTOP,
        "feat-x",
        "desktop",
        "ended",
        "2026-01-01T00:00:00Z",
    )
    .await;
    db
}

fn config(dir: &Path) -> agent_hub::config::Config {
    common::state::config(dir)
}

async fn open(dir: &Path) -> AppState {
    AppState::open(config(dir)).await.expect("open")
}

fn read(path: impl AsRef<Path>) -> Vec<u8> {
    std::fs::read(path.as_ref()).unwrap_or_else(|err| panic!("{}: {err}", path.as_ref().display()))
}

fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).expect("read dir").flatten() {
            let path = entry.path();
            out.push(path.strip_prefix(dir).expect("under").to_path_buf());
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    out.sort();
    out
}

fn key(member: &str, agent: &str) -> ParticipantKey {
    ParticipantKey {
        member: member.to_string(),
        agent: agent.to_string(),
    }
}

#[tokio::test]
async fn a_fresh_directory_opens_at_the_latest_with_no_backup() {
    let dir = TempDir::new("idm-fresh");
    let state = open(&dir).await;
    assert_eq!(
        state.schema_version,
        agent_hub::store::schema::SUPPORTED_MAX
    );
    assert!(
        !dir.join("backups").exists(),
        "a fresh store has nothing to copy aside"
    );
}

#[tokio::test]
async fn same_name_sessions_merge_into_one_shared_session() {
    let dir = TempDir::new("idm-merge");
    let db = merge_fixture(&dir).await;
    event(&db, &id(1), "signal", "laptop/opencode", Some(OPENCODE)).await;
    drop(db);

    let state = open(&dir).await;
    let held = sessions::list(&state.db, PROJECT).await.expect("list");
    assert_eq!(held.len(), 1, "one feat-x: {held:?}");
    assert_eq!(held[0].id, SURVIVOR);
    assert_eq!(held[0].status, "active");
    let mut joined: Vec<(String, String)> = sessions::participants(&state.db, SURVIVOR)
        .await
        .expect("participants")
        .into_iter()
        .map(|p| (p.member, p.agent))
        .collect();
    joined.sort();
    assert_eq!(
        joined,
        vec![
            ("desktop".to_string(), String::new()),
            ("laptop".to_string(), "claude-code".to_string()),
            ("laptop".to_string(), "opencode".to_string()),
        ]
    );
    assert_eq!(
        texts(
            &state.db,
            &format!("SELECT session_id FROM events WHERE id = '{}'", id(1))
        )
        .await,
        vec![SURVIVOR.to_string()],
        "events point at the survivor"
    );
    let resolved = sessions::lookup_id(&state.db, OPENCODE)
        .await
        .expect("lookup")
        .expect("an alias");
    assert_eq!(resolved.id, SURVIVOR, "a loser id resolves to the survivor");
    let backups = std::fs::read_dir(dir.join("backups"))
        .expect("backups")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("pre-migration-v27-")
        })
        .count();
    assert_eq!(backups, 1, "one full backup set before migrating");
}

#[tokio::test]
async fn a_member_id_with_a_slash_places_before_a_split() {
    let dir = TempDir::new("idm-slash");
    let db = store_at(&dir, 27).await;
    agent(&db, "claude-code/laptop").await;
    sql(
        &db,
        &format!(
            "INSERT INTO projects(id, display_name, created_at) VALUES ('{PROJECT}', 'H', 'x')"
        ),
    )
    .await;
    session(
        &db,
        "s-a",
        "a",
        "claude-code/laptop",
        "ended",
        "2026-01-01T00:00:00Z",
    )
    .await;
    session(
        &db,
        "s-b",
        "b",
        "claude-code/laptop/x",
        "ended",
        "2026-01-01T00:00:00Z",
    )
    .await;
    drop(db);
    let state = open(&dir).await;
    let a = sessions::participants(&state.db, "s-a").await.expect("a");
    let b = sessions::participants(&state.db, "s-b").await.expect("b");
    assert_eq!(
        (a[0].member.as_str(), a[0].agent.as_str()),
        ("claude-code/laptop", "")
    );
    assert_eq!(
        (b[0].member.as_str(), b[0].agent.as_str()),
        ("claude-code/laptop", "x")
    );
}

#[tokio::test]
async fn a_kv_conflict_keeps_the_survivor_and_relocates_the_loser() {
    let dir = TempDir::new("idm-kv");
    let db = merge_fixture(&dir).await;
    drop(db);
    put(&dir, SURVIVOR, "/kv/plan", b"survivor plan").await;
    put(&dir, OPENCODE, "/kv/plan", b"loser plan").await;

    let state = open(&dir).await;
    let brain = state
        .brain
        .open_existing(PROJECT, SURVIVOR)
        .await
        .expect("open")
        .expect("survivor brain");
    assert_eq!(
        brain.get("/kv/plan").await.expect("get"),
        Some(b"survivor plan".to_vec())
    );
    assert_eq!(
        brain
            .get(&format!("/kv/merged/{OPENCODE}/plan"))
            .await
            .expect("get"),
        Some(b"loser plan".to_vec())
    );
    let docs = texts(
        &state.db,
        "SELECT session_id || ' ' || ref_id FROM search_docs WHERE type = 'brain' ORDER BY ref_id",
    )
    .await;
    assert_eq!(
        docs,
        vec![
            format!("{SURVIVOR} /kv/merged/{OPENCODE}/plan"),
            format!("{SURVIVOR} /kv/plan"),
        ],
        "both paths are found under the survivor only"
    );
    assert!(
        dir.join(format!("merged/{PROJECT}/{OPENCODE}.db"))
            .is_file()
    );
    assert!(
        !dir.join(format!("sessions/{PROJECT}/{OPENCODE}.db"))
            .exists()
    );
}

#[tokio::test]
async fn a_taken_conflict_root_moves_to_the_next_free_one() {
    let dir = TempDir::new("idm-root");
    let db = merge_fixture(&dir).await;
    drop(db);
    put(&dir, SURVIVOR, "/kv/plan", b"mine").await;
    put(
        &dir,
        SURVIVOR,
        &format!("/kv/merged/{OPENCODE}/plan"),
        b"held",
    )
    .await;
    put(&dir, OPENCODE, "/kv/plan", b"theirs").await;

    let state = open(&dir).await;
    let brain = state
        .brain
        .open_existing(PROJECT, SURVIVOR)
        .await
        .expect("open")
        .expect("brain");
    assert_eq!(
        brain
            .get(&format!("/kv/merged-{OPENCODE}/plan"))
            .await
            .expect("get"),
        Some(b"theirs".to_vec())
    );
    assert_eq!(
        brain
            .get(&format!("/kv/merged/{OPENCODE}/plan"))
            .await
            .expect("get"),
        Some(b"held".to_vec()),
        "the survivor's own key is untouched"
    );
}

#[tokio::test]
async fn file_and_directory_collisions_relocate_under_a_free_root() {
    let dir = TempDir::new("idm-fs");
    let db = merge_fixture(&dir).await;
    drop(db);
    put(&dir, SURVIVOR, "/fs/a/b", b"survivor b").await;
    put(&dir, SURVIVOR, "/fs/c", b"survivor c").await;
    put(&dir, SURVIVOR, "/fs/merged", b"a file named merged").await;
    put(&dir, OPENCODE, "/fs/a", b"loser a").await;
    put(&dir, OPENCODE, "/fs/c/d", b"loser d").await;

    let state = open(&dir).await;
    let brain = state
        .brain
        .open_existing(PROJECT, SURVIVOR)
        .await
        .expect("open")
        .expect("brain");
    let root = format!("merged-{OPENCODE}");
    assert_eq!(
        brain.get("/fs/a/b").await.expect("get"),
        Some(b"survivor b".to_vec())
    );
    assert_eq!(
        brain.get("/fs/c").await.expect("get"),
        Some(b"survivor c".to_vec())
    );
    assert_eq!(
        brain.get("/fs/merged").await.expect("get"),
        Some(b"a file named merged".to_vec())
    );
    assert_eq!(
        brain.get(&format!("/fs/{root}/a")).await.expect("get"),
        Some(b"loser a".to_vec())
    );
    assert_eq!(
        brain.get(&format!("/fs/{root}/c/d")).await.expect("get"),
        Some(b"loser d".to_vec())
    );
    let mapped = texts(
        &state.db,
        &format!(
            "SELECT src || ' ' || action || ' ' || COALESCE(dest, '-') FROM identity_merge_paths
              WHERE loser_session_id = '{OPENCODE}' AND kind <> 'dir' ORDER BY src"
        ),
    )
    .await;
    assert_eq!(
        mapped,
        vec![
            format!("/fs/a relocate /fs/{root}/a"),
            format!("/fs/c/d relocate /fs/{root}/c/d"),
        ]
    );
}

#[tokio::test]
async fn a_loser_with_no_brain_file_merges_as_empty() {
    let dir = TempDir::new("idm-missing");
    let db = merge_fixture(&dir).await;
    drop(db);
    put(&dir, SURVIVOR, "/kv/plan", b"mine").await;
    let state = open(&dir).await;
    let stages = texts(
        &state.db,
        "SELECT stage FROM identity_merges ORDER BY loser_session_id",
    )
    .await;
    assert_eq!(stages, vec!["done", "done"]);
    assert!(!dir.join("merged").exists(), "nothing to archive");
}

#[tokio::test]
async fn a_restart_at_closed_finishes_a_missing_loser() {
    let dir = TempDir::new("idm-missing-closed");
    let db = merge_fixture(&dir).await;
    drop(db);
    let db = open_engine(&dir.join("hub.db")).await.expect("open");
    agent_hub::store::migrate_in(&db, Some(&dir))
        .await
        .expect("migrate");
    let err = migrate::run_merges(&db, &dir, Some(FailPoint::AfterClose))
        .await
        .expect_err("stopped");
    assert!(err.to_string().contains("AfterClose"));
    drop(db);
    let state = open(&dir).await;
    let stages = texts(&state.db, "SELECT stage FROM identity_merges").await;
    assert!(stages.iter().all(|stage| stage == "done"), "{stages:?}");
}

#[tokio::test]
async fn every_fail_point_finishes_on_the_next_open_and_then_holds_still() {
    for point in [
        FailPoint::AfterMap,
        FailPoint::AfterCopy,
        FailPoint::AfterVerify,
        FailPoint::AfterClose,
        FailPoint::AfterArchiveRename,
        FailPoint::AfterArchive,
    ] {
        let dir = TempDir::new("idm-fail");
        let db = merge_fixture(&dir).await;
        drop(db);
        put(&dir, SURVIVOR, "/kv/plan", b"survivor").await;
        put(&dir, OPENCODE, "/kv/plan", b"loser").await;
        put(&dir, OPENCODE, "/fs/notes.md", b"notes").await;

        let db = open_engine(&dir.join("hub.db")).await.expect("open");
        agent_hub::store::migrate_in(&db, Some(&dir))
            .await
            .expect("migrate");
        migrate::run_merges(&db, &dir, Some(point))
            .await
            .expect_err("the fail point stops the worker");
        drop(db);

        let state = open(&dir).await;
        let stages = texts(&state.db, "SELECT stage FROM identity_merges").await;
        assert!(
            stages.iter().all(|stage| stage == "done"),
            "{point:?}: {stages:?}"
        );
        let brain = state
            .brain
            .open_existing(PROJECT, SURVIVOR)
            .await
            .expect("open")
            .expect("brain");
        assert_eq!(
            brain
                .get(&format!("/kv/merged/{OPENCODE}/plan"))
                .await
                .expect("get"),
            Some(b"loser".to_vec()),
            "{point:?}"
        );
        assert_eq!(
            brain.get("/fs/notes.md").await.expect("get"),
            Some(b"notes".to_vec()),
            "{point:?}"
        );
        let copied = texts(
            &state.db,
            "SELECT COUNT(*) FROM identity_merge_paths WHERE action <> 'skip'",
        )
        .await;
        drop(brain);
        drop(state);

        let survivor = dir.join(format!("sessions/{PROJECT}/{SURVIVOR}.db"));
        let archive = dir.join(format!("merged/{PROJECT}/{OPENCODE}.db"));
        let before = (read(&survivor), read(&archive));
        let again = open(&dir).await;
        assert_eq!(
            texts(
                &again.db,
                "SELECT COUNT(*) FROM identity_merge_paths WHERE action <> 'skip'"
            )
            .await,
            copied,
            "{point:?}: the map is never recomputed"
        );
        drop(again);
        assert_eq!(
            (read(&survivor), read(&archive)),
            before,
            "{point:?}: a further open changes neither file"
        );
    }
}

#[tokio::test]
async fn notify_cursors_collide_at_the_lowest_and_subscriptions_merge() {
    let dir = TempDir::new("idm-cursors");
    let db = merge_fixture(&dir).await;
    sql(
        &db,
        &format!(
            "INSERT INTO agent_notify_cursors(agent, session_id, last_seen_event_id, updated_at) VALUES
               ('laptop/claude', '{SURVIVOR}', '{}', 'x'),
               ('laptop/claude', '', '{}', 'x'),
               ('laptop', '', '{}', 'x');
             INSERT INTO notify_subscriptions(id, agent, project_id, kinds, last_seen_event_id, created_at, actor_filter)
             VALUES ('sub-1', 'laptop/claude', NULL, 'signal,answer', '{}', 'x', NULL),
                    ('sub-2', 'laptop/claude', NULL, 'answer,signal', '{}', 'x', NULL),
                    ('sub-3', 'desktop', NULL, 'signal', '{}', 'x', 'stray');",
            id(50),
            id(40),
            id(30),
            id(20),
            id(10),
            id(5),
        ),
    )
    .await;
    drop(db);

    let state = open(&dir).await;
    let cursors = texts(
        &state.db,
        "SELECT member_id || '|' || agent || '|' || session_id || '|' || last_seen_event_id
           FROM notify_cursors ORDER BY member_id, agent, session_id",
    )
    .await;
    assert_eq!(
        cursors,
        vec![
            format!("laptop|||{}", id(30)),
            format!("laptop|claude-code||{}", id(40)),
            format!("laptop|claude-code|{SURVIVOR}|{}", id(50)),
        ]
    );
    let subs = texts(
        &state.db,
        "SELECT id || '|' || last_seen_event_id || '|' || COALESCE(author_filter, '-')
           FROM notify_subscriptions ORDER BY id",
    )
    .await;
    assert_eq!(
        subs,
        vec![
            format!("sub-1|{}|-", id(10)),
            format!("sub-3|{}|{{\"label\":\"stray\"}}", id(5)),
        ],
        "equal subscriptions merge at the lower watermark; an unplaced filter keeps its label"
    );
}

#[tokio::test]
async fn an_unplaced_owner_stops_the_start_and_changes_nothing() {
    let dir = TempDir::new("idm-stop");
    let db = store_at(&dir, 27).await;
    sql(
        &db,
        &format!(
            "INSERT INTO projects(id, display_name, created_at) VALUES ('{PROJECT}', 'H', 'x')"
        ),
    )
    .await;
    session(
        &db,
        "s-stray",
        "work",
        "stray",
        "ended",
        "2026-01-01T00:00:00Z",
    )
    .await;
    drop(db);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    }
    let hub_before = read(dir.join("hub.db"));
    let tree_before = tree(&dir);

    let err = AppState::open(config(&dir))
        .await
        .err()
        .expect("the start stops");
    assert!(migrate::is_stop(&err), "a stop: {err}");
    let message = err.to_string();
    assert!(
        message.contains("'stray' (1 session)") && message.contains("Nothing was changed."),
        "{message}"
    );
    assert_eq!(
        read(dir.join("hub.db")),
        hub_before,
        "hub.db is byte-identical"
    );
    assert_eq!(tree(&dir), tree_before, "no entry was added or removed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    std::fs::write(
        dir.join("identity-map.toml"),
        "[owners]\n\"stray\" = \"human\"\n",
    )
    .expect("map");
    let state = open(&dir).await;
    assert_eq!(
        state.schema_version,
        agent_hub::store::schema::SUPPORTED_MAX,
        "a mapped owner opens"
    );
}

#[tokio::test]
async fn a_v20_store_is_planned_on_a_staged_copy() {
    let dir = TempDir::new("idm-v20");
    let db = store_at(&dir, 20).await;
    sql(
        &db,
        &format!(
            "INSERT INTO projects(id, display_name, created_at) VALUES ('{PROJECT}', 'H', 'x')"
        ),
    )
    .await;
    sql(
        &db,
        "INSERT INTO agents(id, display_name, personal_project_id, created_at) VALUES ('laptop', 'L', 'space-l', 'x')",
    )
    .await;
    session(&db, "s-1", "work", "stray", "ended", "2026-01-01T00:00:00Z").await;
    session(
        &db,
        "s-2",
        "other",
        "laptop/claude",
        "ended",
        "2026-01-01T00:00:00Z",
    )
    .await;
    drop(db);
    let before = read(dir.join("hub.db"));
    let err = AppState::open(config(&dir)).await.err().expect("stops");
    assert!(
        migrate::is_stop(&err) && err.to_string().contains("'stray'"),
        "{err}"
    );
    assert_eq!(
        read(dir.join("hub.db")),
        before,
        "the original is untouched"
    );
    assert!(!dir.join("backups").exists(), "the staged copy is gone");

    let plan = migrate::check(&dir, &dir.join("backups"))
        .await
        .expect("check");
    assert_eq!(plan.from_version, 20);
    assert!(!plan.is_clean());

    std::fs::write(
        dir.join("identity-map.toml"),
        "[owners]\n\"stray\" = { member = \"laptop\" }\n",
    )
    .expect("map");
    let state = open(&dir).await;
    assert_eq!(
        state.schema_version,
        agent_hub::store::schema::SUPPORTED_MAX
    );
    let stopped = schema_version(&state.db).await.expect("version");
    assert_eq!(stopped, agent_hub::store::schema::SUPPORTED_MAX);
}

#[tokio::test]
async fn a_v27_comment_audience_is_rewritten_and_reaches_the_owned_session() {
    let dir = TempDir::new("idm-audience");
    let db = merge_fixture(&dir).await;
    sql(
        &db,
        &format!(
            "INSERT INTO events(id, project_id, kind, actor, summary, payload, needs_action, created_at)
             VALUES ('{}', '{PROJECT}', 'comment', 'desktop', 'comment on x',
                     '{{\"target\":\"artifact\",\"audience\":[\"laptop/claude\",\"stray\"]}}', 0, 'x')",
            id(7)
        ),
    )
    .await;
    drop(db);
    let state = open(&dir).await;
    let payload = texts(
        &state.db,
        &format!("SELECT payload FROM events WHERE id = '{}'", id(7)),
    )
    .await;
    let payload: serde_json::Value = serde_json::from_str(&payload[0]).expect("json");
    assert_eq!(
        payload["audience"]["labels"],
        serde_json::json!(["laptop/claude", "stray"])
    );
    assert_eq!(
        payload["audience"]["sessions"],
        serde_json::json!([SURVIVOR])
    );
    let page =
        notifications::attention(&state.db, &key("laptop", "claude-code"), SURVIVOR, 10, None)
            .await
            .expect("attention");
    assert!(
        page.events.iter().any(|event| event.id == id(7)),
        "the participant in the owned session gets the comment"
    );
}

#[tokio::test]
async fn a_reply_asked_in_a_loser_reaches_its_participant_after_the_upgrade() {
    let dir = TempDir::new("idm-reply");
    let db = merge_fixture(&dir).await;
    answered(&db, &id(20), &id(21), "laptop/opencode", OPENCODE).await;
    sql(
        &db,
        &format!(
            "INSERT INTO agent_notify_cursors(agent, session_id, last_seen_event_id, updated_at)
             VALUES ('laptop/opencode', '{OPENCODE}', '{}', 'x')",
            id(10)
        ),
    )
    .await;
    drop(db);
    let state = open(&dir).await;
    let page = notifications::attention(&state.db, &key("laptop", "opencode"), SURVIVOR, 10, None)
        .await
        .expect("attention");
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.event_id.clone())
            .collect::<Vec<_>>(),
        vec![id(20)]
    );
}

#[tokio::test]
async fn a_staggered_cursor_delivers_the_merged_answer_once() {
    let dir = TempDir::new("idm-backlog");
    let db = merge_fixture(&dir).await;
    // The opencode loser's question was answered at 90; the claude
    // participant's cursor already stands at 100.
    answered(&db, &id(80), &id(90), "laptop/opencode", OPENCODE).await;
    sql(
        &db,
        &format!(
            "INSERT INTO agent_notify_cursors(agent, session_id, last_seen_event_id, updated_at)
             VALUES ('laptop/claude', '{SURVIVOR}', '{}', 'x')",
            id(100)
        ),
    )
    .await;
    drop(db);
    let state = open(&dir).await;
    let me = key("laptop", "claude-code");
    let first = notifications::attention(&state.db, &me, SURVIVOR, 10, None)
        .await
        .expect("attention");
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.event_id.clone())
            .collect::<Vec<_>>(),
        vec![id(80)],
        "the backlog delivers the answer below the cursor"
    );
    assert!(
        notifications::settle(&state.db, &first.settle)
            .await
            .expect("settle")
    );
    let again = notifications::attention(&state.db, &me, SURVIVOR, 10, None)
        .await
        .expect("attention");
    assert!(again.items.is_empty(), "and only once");
}

#[tokio::test]
async fn backup_restore_and_check_carry_the_merged_brains() {
    let dir = TempDir::new("idm-backup");
    let db = merge_fixture(&dir).await;
    drop(db);
    put(&dir, SURVIVOR, "/kv/plan", b"survivor").await;
    put(&dir, OPENCODE, "/kv/plan", b"loser").await;
    let state = open(&dir).await;
    let archive = format!("merged/{PROJECT}/{OPENCODE}.db");

    let out = TempDir::new("idm-backup-out");
    let target = out.join("set");
    agent_hub::ops::backup_serving(
        &dir,
        agent_hub::ops::Serving {
            db: &state.db,
            brain: &state.brain,
            knowledge: &state.knowledge,
        },
        &target,
    )
    .await
    .expect("online backup");
    assert!(target.join(&archive).is_file(), "the set holds the archive");
    let manifest = std::fs::read_to_string(target.join("manifest.json")).expect("manifest");
    assert!(manifest.contains(&archive));
    drop(state);

    let checked = agent_hub::ops::check(&target).await.expect("check");
    assert!(checked.problems.is_empty(), "{:?}", checked.problems);
    let restored = TempDir::new("idm-restored");
    let into = restored.join("data");
    agent_hub::ops::restore(&target, &into, false)
        .await
        .expect("restore");
    assert!(into.join(&archive).is_file(), "restore carries the archive");
}

#[tokio::test]
async fn rolling_back_to_the_pre_upgrade_set_brings_back_a_deleted_artifact() {
    let dir = TempDir::new("idm-rollback");
    let db = merge_fixture(&dir).await;
    let rel =
        agent_hub::blob::write(&dir, PROJECT, "art-1", 1, "markdown", b"# kept").expect("blob");
    sql(
        &db,
        &format!(
            "INSERT INTO artifacts(id, project_id, title, kind, current_ver, path, size_bytes, created_at, updated_at)
             VALUES ('art-1', '{PROJECT}', 'Kept', 'markdown', 1, '{rel}', 6, 'x', 'x');
             INSERT INTO artifact_versions(artifact_id, version, title, kind, size_bytes, path, created_at)
             VALUES ('art-1', 1, 'Kept', 'markdown', 6, '{rel}', 'x');"
        ),
    )
    .await;
    drop(db);

    let state = open(&dir).await;
    agent_hub::store::artifacts::delete(&state.db, &dir, &common::seed::writer("human"), "art-1")
        .await
        .expect("delete after the upgrade");
    assert!(!dir.join(&rel).exists());
    drop(state);

    let set = std::fs::read_dir(dir.join("backups"))
        .expect("backups")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("pre-migration-"))
        })
        .expect("the pre-upgrade set");
    let aside = TempDir::new("idm-rollback-set");
    let copy = aside.join("set");
    std::fs::rename(&set, &copy).expect("move the set out of the data directory");
    agent_hub::ops::restore(&copy, &dir, true)
        .await
        .expect("restore the set whole");
    let db = open_engine(&dir.join("hub.db")).await.expect("open");
    assert_eq!(schema_version(&db).await.expect("version"), 27);
    assert_eq!(
        texts(&db, "SELECT id FROM artifacts").await,
        vec!["art-1".to_string()]
    );
    assert!(dir.join(&rel).is_file(), "the blob is back");
    assert_eq!(
        texts(&db, "SELECT COUNT(*) FROM sessions").await,
        vec!["3".to_string()]
    );
}
