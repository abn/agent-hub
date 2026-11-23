//! The members upgrade: preflight plan, placement of old owner labels, and
//! the journalled brain merge.
//!
//! `check` is read-only and backs `agent-hub migrate --check`. `run_merges`
//! is the startup merge worker; `AppState::open` runs it to completion before
//! the hub serves.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use turso::{Connection, Row, Value};

use crate::author::Origin;
use crate::error::{Error, Result};

/// The operator's owner map, read from the data directory.
pub const MAP_FILE: &str = "identity-map.toml";

/// The version the members upgrade starts from.
const FROM: i64 = 27;

/// Where an old owner label lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    Participant {
        member: String,
        agent: String,
    },
    Local {
        agent: String,
    },
    Human {
        origin: Origin,
    },
    /// The operator map drops it (cursors and subscriptions only).
    Drop,
    Unplaced,
}

impl Placement {
    /// The delivery key a placed label keys cursors and subscriptions by.
    fn key(&self) -> Option<(String, String)> {
        match self {
            Self::Participant { member, agent } => Some((member.clone(), agent.clone())),
            Self::Local { agent } => Some((crate::author::LOCAL_MEMBER.to_string(), agent.clone())),
            _ => None,
        }
    }
}

/// One label the store holds and where it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedLabel {
    pub label: String,
    pub placement: Placement,
    pub sessions: usize,
    pub cursors: usize,
    pub subscriptions: usize,
}

/// Live sessions of one name in one project, merged into the survivor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeGroup {
    pub project_id: String,
    pub session_name: String,
    pub survivor: String,
    pub losers: Vec<String>,
}

/// The preflight plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The version the store is at.
    pub from_version: i64,
    pub labels: Vec<PlacedLabel>,
    pub groups: Vec<MergeGroup>,
    /// Each stop's message, worded as the start prints it. Empty when clean.
    pub stops: Vec<String>,
}

impl Plan {
    /// Whether the upgrade may proceed.
    pub fn is_clean(&self) -> bool {
        self.stops.is_empty()
    }

    fn empty(from_version: i64) -> Self {
        Self {
            from_version,
            labels: Vec::new(),
            groups: Vec::new(),
            stops: Vec::new(),
        }
    }
}

/// Whether an error is a preflight stop: the store holds something the
/// upgrade cannot place, and nothing was changed. A start that sees one exits
/// 78.
pub fn is_stop(err: &Error) -> bool {
    matches!(err, Error::Config(message) if message.starts_with(STOP_MARK))
}

/// Every stop message starts with this, so [`is_stop`] can tell one from any
/// other configuration error.
const STOP_MARK: &str = "the store cannot be upgraded: ";

/// A stop with one message.
pub(crate) fn stop_with(message: String) -> Error {
    Error::Config(format!("{STOP_MARK}{message}"))
}

/// The error a start gives for the plan's stops.
pub fn stop_error(plan: &Plan) -> Error {
    Error::Config(format!("{STOP_MARK}{}", plan.stops.join("\n")))
}

// The operator map.

#[derive(Debug, Clone, PartialEq, Eq)]
enum MapValue {
    Member { member: String, agent: String },
    Human,
    Local,
    Drop,
}

/// `<data_dir>/identity-map.toml`, `[owners]`: label to placement.
#[derive(Debug, Default)]
pub(crate) struct OwnerMap {
    entries: BTreeMap<String, MapValue>,
    data_dir: Option<PathBuf>,
}

impl OwnerMap {
    /// Read the map under `data_dir`. A missing file, or no data directory,
    /// is an empty map.
    pub(crate) fn load(data_dir: Option<&Path>) -> Result<Self> {
        let mut map = Self {
            entries: BTreeMap::new(),
            data_dir: data_dir.map(Path::to_path_buf),
        };
        let Some(dir) = data_dir else {
            return Ok(map);
        };
        let path = dir.join(MAP_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(map),
            Err(err) => return Err(err.into()),
        };
        let bad = |why: String| Error::Config(format!("{} is not valid: {why}", path.display()));
        let parsed: toml::Table = text.parse().map_err(|err| bad(format!("{err}")))?;
        let Some(owners) = parsed.get("owners") else {
            return Ok(map);
        };
        let owners = owners
            .as_table()
            .ok_or_else(|| bad("[owners] must be a table".to_string()))?;
        for (label, value) in owners {
            let entry = match value {
                toml::Value::String(word) => match word.as_str() {
                    "human" => MapValue::Human,
                    "local" => MapValue::Local,
                    "drop" => MapValue::Drop,
                    other => {
                        return Err(bad(format!(
                            "'{label}' maps to '{other}'; use a member table, \"human\", \"local\" or \"drop\""
                        )));
                    }
                },
                toml::Value::Table(table) => {
                    let member = table
                        .get("member")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| bad(format!("'{label}' needs a member")))?;
                    let agent = table
                        .get("agent")
                        .and_then(toml::Value::as_str)
                        .map(crate::agent_name::canonical)
                        .unwrap_or_default();
                    MapValue::Member {
                        member: member.to_string(),
                        agent,
                    }
                }
                _ => return Err(bad(format!("'{label}' has a value of the wrong type"))),
            };
            map.entries.insert(label.clone(), entry);
        }
        Ok(map)
    }

    fn dir_label(&self) -> String {
        match &self.data_dir {
            Some(dir) => dir.join(MAP_FILE).display().to_string(),
            None => format!("<data_dir>/{MAP_FILE}"),
        }
    }
}

/// Place one old owner label (spec order: map, reserved words, registered
/// id, `<member>/<suffix>` split at the last `/`).
fn place(label: &str, members: &HashSet<String>, map: &OwnerMap) -> Placement {
    if let Some(value) = map.entries.get(label) {
        return match value {
            MapValue::Member { member, agent } => Placement::Participant {
                member: member.clone(),
                agent: agent.clone(),
            },
            MapValue::Human => Placement::Human {
                origin: Origin::Call,
            },
            MapValue::Local => Placement::Local {
                agent: String::new(),
            },
            MapValue::Drop => Placement::Drop,
        };
    }
    match label {
        crate::author::HUMAN_KEY => {
            return Placement::Human {
                origin: Origin::Call,
            };
        }
        crate::author::HUB_KEY => {
            return Placement::Human {
                origin: Origin::Hub,
            };
        }
        crate::author::LOCAL_MEMBER => {
            return Placement::Local {
                agent: String::new(),
            };
        }
        _ => {}
    }
    if members.contains(label) {
        return Placement::Participant {
            member: label.to_string(),
            agent: String::new(),
        };
    }
    if let Some((prefix, suffix)) = label.rsplit_once('/')
        && members.contains(prefix)
        && crate::principal::is_declared_name(suffix)
    {
        return Placement::Participant {
            member: prefix.to_string(),
            agent: crate::agent_name::canonical(suffix),
        };
    }
    Placement::Unplaced
}

// Reading the v27 store.

#[derive(Debug, Clone)]
struct OldSession {
    id: String,
    project_id: String,
    session_name: String,
    agent: String,
    status: String,
    brain_path: String,
    created_at: String,
    last_activity: String,
    deleted_at: Option<String>,
    forked_from: Option<String>,
    adopted_from: Option<String>,
    handoff: Option<String>,
}

/// Everything the data step decides before it writes.
struct Planned {
    plan: Plan,
    placements: HashMap<String, Placement>,
    members: HashSet<String>,
    sessions: Vec<OldSession>,
    /// Loser id to survivor id.
    survivor_of: HashMap<String, String>,
}

fn text(row: &Row, index: usize) -> Result<String> {
    opt(row, index)?.ok_or_else(|| Error::Engine(format!("expected text in column {index}")))
}

fn opt(row: &Row, index: usize) -> Result<Option<String>> {
    match row.get_value(index).map_err(super::engine)? {
        Value::Text(value) => Ok(Some(value)),
        Value::Null => Ok(None),
        Value::Integer(value) => Ok(Some(value.to_string())),
        other => Err(Error::Engine(format!(
            "expected text in column {index}, found {other:?}"
        ))),
    }
}

fn int(row: &Row, index: usize) -> Result<i64> {
    match row.get_value(index).map_err(super::engine)? {
        Value::Integer(value) => Ok(value),
        Value::Null => Ok(0),
        other => Err(Error::Engine(format!(
            "expected an integer in column {index}, found {other:?}"
        ))),
    }
}

async fn label_counts(conn: &Connection, sql: &str) -> Result<BTreeMap<String, usize>> {
    let mut rows = conn.query(sql, ()).await.map_err(super::engine)?;
    let mut out = BTreeMap::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        out.insert(text(&row, 0)?, int(&row, 1)? as usize);
    }
    Ok(out)
}

fn plural(count: usize, one: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {one}s")
    }
}

/// Plan the upgrade over a store at v27, reading only.
async fn plan_on(conn: &Connection, map: &OwnerMap) -> Result<Planned> {
    let mut members = HashSet::new();
    {
        let mut rows = conn
            .query("SELECT id FROM agents", ())
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            members.insert(text(&row, 0)?);
        }
    }
    let mut sessions = Vec::new();
    {
        let mut rows = conn
            .query(
                "SELECT id, project_id, session_name, agent, status, brain_path, created_at,
                        last_activity, deleted_at, forked_from, adopted_from, handoff
                   FROM sessions ORDER BY id",
                (),
            )
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            sessions.push(OldSession {
                id: text(&row, 0)?,
                project_id: text(&row, 1)?,
                session_name: text(&row, 2)?,
                agent: text(&row, 3)?,
                status: text(&row, 4)?,
                brain_path: text(&row, 5)?,
                created_at: text(&row, 6)?,
                last_activity: text(&row, 7)?,
                deleted_at: opt(&row, 8)?,
                forked_from: opt(&row, 9)?,
                adopted_from: opt(&row, 10)?,
                handoff: opt(&row, 11)?,
            });
        }
    }
    let feed = label_counts(
        conn,
        "SELECT agent, COUNT(*) FROM agent_feed_cursors GROUP BY agent",
    )
    .await?;
    let notify = label_counts(
        conn,
        "SELECT agent, COUNT(*) FROM agent_notify_cursors GROUP BY agent",
    )
    .await?;
    let subs = label_counts(
        conn,
        "SELECT agent, COUNT(*) FROM notify_subscriptions GROUP BY agent",
    )
    .await?;

    let mut session_counts: BTreeMap<String, usize> = BTreeMap::new();
    for session in &sessions {
        *session_counts.entry(session.agent.clone()).or_default() += 1;
    }
    let labels: BTreeSet<String> = session_counts
        .keys()
        .chain(feed.keys())
        .chain(notify.keys())
        .chain(subs.keys())
        .cloned()
        .collect();

    let mut stops = Vec::new();
    for (label, value) in &map.entries {
        if let MapValue::Member { member, .. } = value
            && !members.contains(member)
        {
            stops.push(format!(
                "{MAP_FILE} maps '{label}' to member '{member}', which is not a member"
            ));
        }
    }
    let mut placements = HashMap::new();
    let mut placed = Vec::new();
    let mut unplaced = Vec::new();
    for label in labels {
        let placement = place(&label, &members, map);
        let sessions_held = session_counts.get(&label).copied().unwrap_or(0);
        let cursors =
            feed.get(&label).copied().unwrap_or(0) + notify.get(&label).copied().unwrap_or(0);
        let subscriptions = subs.get(&label).copied().unwrap_or(0);
        match placement {
            Placement::Unplaced => {
                let mut held = Vec::new();
                if sessions_held > 0 {
                    held.push(plural(sessions_held, "session"));
                }
                if cursors > 0 {
                    held.push(plural(cursors, "cursor"));
                }
                if subscriptions > 0 {
                    held.push(plural(subscriptions, "subscription"));
                }
                unplaced.push(format!("'{label}' ({})", held.join(", ")));
            }
            Placement::Drop if sessions_held > 0 => stops.push(format!(
                "{MAP_FILE} drops '{label}', which owns {}; sessions cannot be dropped, map it to a member",
                plural(sessions_held, "session")
            )),
            _ => {}
        }
        placements.insert(label.clone(), placement.clone());
        placed.push(PlacedLabel {
            label,
            placement,
            sessions: sessions_held,
            cursors,
            subscriptions,
        });
    }
    if !unplaced.is_empty() {
        stops.insert(
            0,
            format!(
                "the store holds owners this release cannot place: {}. Map them in {} under [owners], then start again. Nothing was changed.",
                unplaced.join(", "),
                map.dir_label()
            ),
        );
    }

    // Live sessions of one name in one project merge into one survivor:
    // active first, then the newest activity, then the smallest id.
    let mut by_name: BTreeMap<(String, String), Vec<&OldSession>> = BTreeMap::new();
    for session in sessions.iter().filter(|s| s.deleted_at.is_none()) {
        by_name
            .entry((session.project_id.clone(), session.session_name.clone()))
            .or_default()
            .push(session);
    }
    let mut groups = Vec::new();
    let mut survivor_of = HashMap::new();
    for ((project_id, session_name), mut held) in by_name {
        if held.len() < 2 {
            continue;
        }
        held.sort_by(|a, b| {
            (b.status == "active")
                .cmp(&(a.status == "active"))
                .then_with(|| b.last_activity.cmp(&a.last_activity))
                .then_with(|| a.id.cmp(&b.id))
        });
        let survivor = held[0].id.clone();
        let losers: Vec<String> = held[1..].iter().map(|s| s.id.clone()).collect();
        for loser in &losers {
            survivor_of.insert(loser.clone(), survivor.clone());
        }
        groups.push(MergeGroup {
            project_id,
            session_name,
            survivor,
            losers,
        });
    }

    Ok(Planned {
        plan: Plan {
            from_version: FROM,
            labels: placed,
            groups,
            stops,
        },
        placements,
        members,
        sessions,
        survivor_of,
    })
}

// The data step.

fn t(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn ot(value: Option<&str>) -> Value {
    value.map_or(Value::Null, t)
}

/// The V28 data step, inside the migration's transaction.
pub(crate) async fn members_step(tx: &super::WriteTx, map: &OwnerMap) -> Result<()> {
    let planned = plan_on(tx, map).await?;
    if !planned.plan.is_clean() {
        return Err(stop_error(&planned.plan));
    }
    let Planned {
        placements,
        members,
        sessions,
        survivor_of,
        ..
    } = planned;
    let surv = |id: &str| -> String {
        survivor_of
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    let now = super::now_rfc3339();

    // Sessions: one row per survivor and per session outside a merge group.
    let mut groups: HashMap<String, Vec<&OldSession>> = HashMap::new();
    for session in &sessions {
        if session.deleted_at.is_none() {
            groups.entry(surv(&session.id)).or_default().push(session);
        }
    }
    for session in &sessions {
        if survivor_of.contains_key(&session.id) {
            continue;
        }
        let (status, last_activity, handoff) = match groups.get(&session.id) {
            Some(group) if group.len() > 1 => {
                let active = group.iter().any(|s| s.status == "active");
                let last = group
                    .iter()
                    .map(|s| s.last_activity.clone())
                    .max()
                    .unwrap_or_default();
                let mut newest: Vec<&&OldSession> =
                    group.iter().filter(|s| s.handoff.is_some()).collect();
                newest.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
                (
                    if active {
                        "active".to_string()
                    } else {
                        session.status.clone()
                    },
                    last,
                    newest.first().and_then(|s| s.handoff.clone()),
                )
            }
            _ => (
                session.status.clone(),
                session.last_activity.clone(),
                session.handoff.clone(),
            ),
        };
        let (kind, member, agent) = match placements.get(&session.agent) {
            Some(Placement::Participant { member, agent }) => {
                ("participant", Some(member.clone()), Some(agent.clone()))
            }
            Some(Placement::Local { agent }) => (
                "local",
                Some(crate::author::LOCAL_MEMBER.to_string()),
                Some(agent.clone()),
            ),
            Some(Placement::Human { .. }) => ("human", None, None),
            _ => ("legacy", None, None),
        };
        let forked = session.forked_from.as_deref().map(&surv);
        tx.execute(
            "INSERT INTO sessions_v28(id, project_id, session_name, parent_id, status, brain_path,
                created_at, last_activity, deleted_at, forked_from, adopted_from, handoff,
                index_pending, started_kind, started_member, started_agent, started_label)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14, ?15)",
            vec![
                t(&session.id),
                t(&session.project_id),
                t(&session.session_name),
                t(&status),
                t(&session.brain_path),
                t(&session.created_at),
                t(&last_activity),
                ot(session.deleted_at.as_deref()),
                ot(forked.as_deref()),
                ot(session.adopted_from.as_deref()),
                ot(handoff.as_deref()),
                t(kind),
                ot(member.as_deref()),
                ot(agent.as_deref()),
                t(&session.agent),
            ],
        )
        .await
        .map_err(super::engine)?;
    }

    // Aliases and merge journal rows for every loser.
    for session in &sessions {
        let Some(survivor) = survivor_of.get(&session.id) else {
            continue;
        };
        tx.execute(
            "INSERT INTO session_aliases(alias_id, session_id, project_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            vec![t(&session.id), t(survivor), t(&session.project_id), t(&now)],
        )
        .await
        .map_err(super::engine)?;
        let present = map
            .data_dir
            .as_ref()
            .is_some_and(|dir| dir.join(&session.brain_path).is_file());
        if !present {
            tracing::info!(
                session_id = %session.id,
                "a merged session has no brain file; it merges as an empty brain"
            );
        }
        tx.execute(
            "INSERT INTO identity_merges(loser_session_id, survivor_session_id, project_id, stage,
                loser_present, conflict_root, archive_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'planned', ?4, NULL, ?5, ?6, ?6)",
            vec![
                t(&session.id),
                t(survivor),
                t(&session.project_id),
                Value::Integer(i64::from(present)),
                t(&archive_path(&session.project_id, &session.id)),
                t(&now),
            ],
        )
        .await
        .map_err(super::engine)?;
    }

    // Participants: the placed owner of each live or ended session, on the
    // survivor for a loser.
    let mut participants: BTreeMap<(String, String, String), (String, String)> = BTreeMap::new();
    for session in sessions.iter().filter(|s| s.deleted_at.is_none()) {
        let Some((member, agent)) = placements.get(&session.agent).and_then(Placement::key) else {
            continue;
        };
        let entry = participants
            .entry((surv(&session.id), member, agent))
            .or_insert_with(|| (session.created_at.clone(), session.last_activity.clone()));
        if session.created_at < entry.0 {
            entry.0 = session.created_at.clone();
        }
        if session.last_activity > entry.1 {
            entry.1 = session.last_activity.clone();
        }
    }
    for ((session_id, member, agent), (joined, seen)) in &participants {
        tx.execute(
            "INSERT INTO session_participants(session_id, member_id, agent, joined_at, last_seen_at, left_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            vec![t(session_id), t(member), t(agent), t(joined), t(seen)],
        )
        .await
        .map_err(super::engine)?;
    }

    // Author backfill on every write-bearing table. The kind stays legacy.
    let mut actors = BTreeSet::new();
    for sql in [
        "SELECT DISTINCT actor FROM events",
        "SELECT DISTINCT actor FROM artifacts WHERE actor IS NOT NULL",
        "SELECT DISTINCT author FROM comments",
        "SELECT DISTINCT author FROM kb_comments",
    ] {
        let mut rows = tx.query(sql, ()).await.map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            if let Some(label) = opt(&row, 0)? {
                actors.insert(label);
            }
        }
    }
    let mut actor_place: HashMap<String, Placement> = HashMap::new();
    for label in actors {
        let placement = placements
            .get(&label)
            .cloned()
            .unwrap_or_else(|| place(&label, &members, map));
        if let Some((member, agent)) = placement.key() {
            for (table, column) in [
                ("events", "actor"),
                ("artifacts", "actor"),
                ("comments", "author"),
                ("kb_comments", "author"),
            ] {
                tx.execute(
                    &format!("UPDATE {table} SET member_id = ?1, agent = ?2 WHERE {column} = ?3"),
                    vec![t(&member), t(&agent), t(&label)],
                )
                .await
                .map_err(super::engine)?;
            }
        }
        actor_place.insert(label, placement);
    }

    // The backlog is decided against the old session ids, so it is read before
    // anything is remapped.
    let backlog = migrated_cursors(tx, &placements, &actor_place, &survivor_of, &groups).await?;

    // Loser ids move to the survivor wherever a row names a session.
    for (loser, survivor) in &survivor_of {
        for sql in [
            "UPDATE events SET session_id = ?2 WHERE session_id = ?1",
            "UPDATE artifacts SET session_id = ?2 WHERE session_id = ?1",
            "UPDATE artifacts SET live_session = ?2 WHERE live_session = ?1",
            "UPDATE comments SET session_id = ?2 WHERE session_id = ?1",
            "UPDATE kb_comments SET session_id = ?2 WHERE session_id = ?1",
            "UPDATE search_docs SET session_id = ?2 WHERE session_id = ?1 AND type <> 'brain'",
            "UPDATE notify_subscriptions SET session_id = ?2 WHERE session_id = ?1",
        ] {
            tx.execute(sql, vec![t(loser), t(survivor)])
                .await
                .map_err(super::engine)?;
        }
        // Brain rows come back from the merged brain once the worker has
        // copied it.
        tx.execute(
            "DELETE FROM search_docs WHERE type = 'brain' AND session_id IN (?1, ?2)",
            vec![t(loser), t(survivor)],
        )
        .await
        .map_err(super::engine)?;
    }

    rewrite_audiences(tx, &sessions, &placements, &actor_place, &survivor_of).await?;

    // Feed cursors: collisions keep the furthest read.
    let mut feed: BTreeMap<(String, String, String), (String, String)> = BTreeMap::new();
    {
        let mut rows = tx
            .query(
                "SELECT project_id, agent, last_seen_event_id, updated_at FROM agent_feed_cursors",
                (),
            )
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            let label = text(&row, 1)?;
            let Some((member, agent)) = placements.get(&label).and_then(Placement::key) else {
                continue;
            };
            let (seen, at) = (text(&row, 2)?, text(&row, 3)?);
            let entry = feed
                .entry((text(&row, 0)?, member, agent))
                .or_insert_with(|| (seen.clone(), at.clone()));
            if seen > entry.0 {
                *entry = (seen, at);
            }
        }
    }
    for ((project, member, agent), (seen, at)) in &feed {
        tx.execute(
            "INSERT INTO feed_cursors(project_id, member_id, agent, last_seen_event_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            vec![t(project), t(member), t(agent), t(seen), t(at)],
        )
        .await
        .map_err(super::engine)?;
    }

    for ((member, agent, session), (seen, at)) in &backlog.cursors {
        tx.execute(
            "INSERT INTO notify_cursors(member_id, agent, session_id, last_seen_event_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            vec![t(member), t(agent), t(session), t(seen), t(at)],
        )
        .await
        .map_err(super::engine)?;
    }
    for (member, agent, session, event) in &backlog.rows {
        tx.execute(
            "INSERT OR IGNORE INTO notify_backlog(member_id, agent, session_id, event_id)
             VALUES (?1, ?2, ?3, ?4)",
            vec![t(member), t(agent), t(session), t(event)],
        )
        .await
        .map_err(super::engine)?;
    }

    migrate_subscriptions(tx, &placements, &members, map).await?;
    Ok(())
}

/// Where a merge group's loser brain is archived, under the data directory.
pub(crate) fn archive_path(project_id: &str, loser_id: &str) -> String {
    format!("merged/{project_id}/{loser_id}.db")
}

type CursorKey = (String, String, String);

struct MigratedCursors {
    cursors: BTreeMap<CursorKey, (String, String)>,
    rows: BTreeSet<(String, String, String, String)>,
}

/// Notify cursors keyed by participant, collisions at the lowest, and the
/// backlog of resolved items a merge brought into a participant's scope below
/// its cursor that its old scope never delivered.
async fn migrated_cursors(
    tx: &super::WriteTx,
    placements: &HashMap<String, Placement>,
    actor_place: &HashMap<String, Placement>,
    survivor_of: &HashMap<String, String>,
    groups: &HashMap<String, Vec<&OldSession>>,
) -> Result<MigratedCursors> {
    let surv = |id: &str| -> String {
        if id.is_empty() {
            String::new()
        } else {
            survivor_of
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string())
        }
    };
    // New key -> (cursor, updated_at, the old (label, session, cursor) rows).
    type Old = Vec<(String, String, String)>;
    let mut keyed: BTreeMap<CursorKey, (String, String, Old)> = BTreeMap::new();
    {
        let mut rows = tx
            .query(
                "SELECT agent, session_id, last_seen_event_id, updated_at FROM agent_notify_cursors",
                (),
            )
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            let label = text(&row, 0)?;
            let Some((member, agent)) = placements.get(&label).and_then(Placement::key) else {
                continue;
            };
            let old_session = text(&row, 1)?;
            let (seen, at) = (text(&row, 2)?, text(&row, 3)?);
            let entry = keyed
                .entry((member, agent, surv(&old_session)))
                .or_insert_with(|| (seen.clone(), at.clone(), Vec::new()));
            if seen < entry.0 {
                entry.0 = seen.clone();
                entry.1 = at;
            }
            entry.2.push((label, old_session, seen));
        }
    }

    let mut rows_out = BTreeSet::new();
    let effective = "COALESCE((SELECT MAX(a.id) FROM events a
         WHERE a.kind = 'answer' AND a.thread_id = e.id AND a.project_id = e.project_id), e.id)";
    for ((member, agent, session), (cursor, _, old)) in &keyed {
        // The items the new scope holds, with their old session and author.
        let mut candidates: Vec<(String, String, Option<String>, String)> = Vec::new();
        if session.is_empty() {
            let mut rows = tx
                .query(
                    &format!(
                        "SELECT e.id, e.actor, e.session_id, {effective}
                           FROM inbox i JOIN events e ON e.id = i.event_id
                          WHERE i.status = 'resolved' AND e.session_id IS NULL"
                    ),
                    (),
                )
                .await
                .map_err(super::engine)?;
            while let Some(row) = rows.next().await.map_err(super::engine)? {
                let actor = text(&row, 1)?;
                let in_scope = actor_place
                    .get(&actor)
                    .and_then(Placement::key)
                    .is_some_and(|(m, a)| &m == member && (&a == agent || a.is_empty()));
                if in_scope {
                    candidates.push((text(&row, 0)?, actor, opt(&row, 2)?, text(&row, 3)?));
                }
            }
        } else {
            let ids: Vec<String> = match groups.get(session) {
                Some(group) => group.iter().map(|s| s.id.clone()).collect(),
                None => vec![session.clone()],
            };
            for id in ids {
                let mut rows = tx
                    .query(
                        &format!(
                            "SELECT e.id, e.actor, e.session_id, {effective}
                               FROM inbox i JOIN events e ON e.id = i.event_id
                              WHERE i.status = 'resolved' AND e.session_id = ?1"
                        ),
                        vec![t(&id)],
                    )
                    .await
                    .map_err(super::engine)?;
                while let Some(row) = rows.next().await.map_err(super::engine)? {
                    candidates.push((
                        text(&row, 0)?,
                        text(&row, 1)?,
                        opt(&row, 2)?,
                        text(&row, 3)?,
                    ));
                }
            }
        }
        for (event_id, actor, old_session, eff) in candidates {
            if eff.as_str() > cursor.as_str() {
                continue;
            }
            let old_session = old_session.unwrap_or_default();
            let delivered = old.iter().any(|(label, scope, seen)| {
                label == &actor && scope == &old_session && eff.as_str() <= seen.as_str()
            });
            if !delivered {
                rows_out.insert((member.clone(), agent.clone(), session.clone(), event_id));
            }
        }
    }
    Ok(MigratedCursors {
        cursors: keyed
            .into_iter()
            .map(|(key, (seen, at, _))| (key, (seen, at)))
            .collect(),
        rows: rows_out,
    })
}

/// A comment or page event's audience, read in either shape: the V27 array
/// of labels, or `{"sessions","participants","labels"}`. New writes keep the
/// label array under `audience` and carry the sessions and participants
/// beside it (`audience_sessions`, `audience_participants`) until every
/// reader takes the object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audience {
    pub sessions: Vec<String>,
    pub participants: Vec<crate::author::ParticipantKey>,
    pub labels: Vec<String>,
}

impl Audience {
    /// Read the audience of an event payload.
    pub fn of(payload: Option<&serde_json::Value>) -> Self {
        let Some(payload) = payload else {
            return Self::default();
        };
        let strings = |value: Option<&serde_json::Value>| -> Vec<String> {
            value
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let keys = |value: Option<&serde_json::Value>| -> Vec<crate::author::ParticipantKey> {
            value
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| serde_json::from_value(item.clone()).ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        match payload.get("audience") {
            Some(serde_json::Value::Object(object)) => Self {
                sessions: strings(object.get("sessions")),
                participants: keys(object.get("participants")),
                labels: strings(object.get("labels")),
            },
            other => Self {
                sessions: strings(payload.get("audience_sessions")),
                participants: keys(payload.get("audience_participants")),
                labels: strings(other),
            },
        }
    }

    /// Whether the audience names a session or a participant.
    pub fn names(
        &self,
        session_id: Option<&str>,
        key: Option<&crate::author::ParticipantKey>,
    ) -> bool {
        session_id.is_some_and(|id| !id.is_empty() && self.sessions.iter().any(|s| s == id))
            || key.is_some_and(|key| self.participants.iter().any(|entry| key.matches(entry)))
    }
}

async fn rewrite_audiences(
    tx: &super::WriteTx,
    sessions: &[OldSession],
    placements: &HashMap<String, Placement>,
    actor_place: &HashMap<String, Placement>,
    survivor_of: &HashMap<String, String>,
) -> Result<()> {
    let mut by_label: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    for session in sessions.iter().filter(|s| s.deleted_at.is_none()) {
        let id = survivor_of
            .get(&session.id)
            .cloned()
            .unwrap_or_else(|| session.id.clone());
        by_label
            .entry((session.project_id.clone(), session.agent.clone()))
            .or_default()
            .insert(id);
    }
    let mut rewrites = Vec::new();
    {
        let mut rows = tx
            .query(
                "SELECT id, project_id, payload FROM events
                  WHERE kind IN ('comment', 'page') AND payload IS NOT NULL",
                (),
            )
            .await
            .map_err(super::engine)?;
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            let id = text(&row, 0)?;
            let project = text(&row, 1)?;
            let Ok(mut payload) = serde_json::from_str::<serde_json::Value>(&text(&row, 2)?) else {
                continue;
            };
            let Some(labels) = payload.get("audience").and_then(|a| a.as_array()).cloned() else {
                continue;
            };
            let mut session_ids = BTreeSet::new();
            let mut keys = BTreeSet::new();
            for label in labels.iter().filter_map(|l| l.as_str()) {
                let placement = placements
                    .get(label)
                    .or_else(|| actor_place.get(label))
                    .cloned()
                    .unwrap_or(Placement::Unplaced);
                if let Some(key) = placement.key() {
                    keys.insert(key);
                    if let Some(ids) = by_label.get(&(project.clone(), label.to_string())) {
                        session_ids.extend(ids.iter().cloned());
                    }
                }
            }
            payload["audience"] = serde_json::json!({
                "sessions": session_ids,
                "participants": keys
                    .into_iter()
                    .map(|(member, agent)| serde_json::json!({"member": member, "agent": agent}))
                    .collect::<Vec<_>>(),
                "labels": labels,
            });
            rewrites.push((id, payload.to_string()));
        }
    }
    for (id, payload) in rewrites {
        tx.execute(
            "UPDATE events SET payload = ?1 WHERE id = ?2",
            vec![t(&payload), t(&id)],
        )
        .await
        .map_err(super::engine)?;
    }
    Ok(())
}

type SubscriptionShape = (
    String,
    String,
    Option<String>,
    Vec<String>,
    Option<String>,
    i64,
    i64,
    Option<String>,
);

async fn migrate_subscriptions(
    tx: &super::WriteTx,
    placements: &HashMap<String, Placement>,
    members: &HashSet<String>,
    map: &OwnerMap,
) -> Result<()> {
    let mut merged: BTreeMap<SubscriptionShape, (String, String, String)> = BTreeMap::new();
    let mut rows = tx
        .query(
            "SELECT id, agent, project_id, kinds, last_seen_event_id, created_at, actor_filter,
                    exclude_self, about_self, session_id
               FROM notify_subscriptions ORDER BY id",
            (),
        )
        .await
        .map_err(super::engine)?;
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        let label = text(&row, 1)?;
        let Some((member, agent)) = placements.get(&label).and_then(Placement::key) else {
            continue;
        };
        let filter = opt(&row, 6)?.and_then(|actor| {
            let filter = match place(&actor, members, map) {
                Placement::Participant { member, agent } => crate::author::AuthorFilter {
                    member: Some(member),
                    agent: (!agent.is_empty()).then_some(agent),
                    ..Default::default()
                },
                Placement::Local { agent } => crate::author::AuthorFilter {
                    member: Some(crate::author::LOCAL_MEMBER.to_string()),
                    agent: (!agent.is_empty()).then_some(agent),
                    ..Default::default()
                },
                Placement::Human { .. } => crate::author::AuthorFilter {
                    kind: Some(crate::author::AuthorKind::Human),
                    ..Default::default()
                },
                Placement::Drop => return None,
                Placement::Unplaced => crate::author::AuthorFilter {
                    label: Some(actor),
                    ..Default::default()
                },
            };
            serde_json::to_string(&filter).ok()
        });
        let mut kinds: Vec<String> = text(&row, 3)?
            .split(',')
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect();
        kinds.sort();
        kinds.dedup();
        let shape = (
            member,
            agent,
            opt(&row, 2)?,
            kinds,
            filter,
            int(&row, 7)?,
            int(&row, 8)?,
            opt(&row, 9)?,
        );
        let (id, seen, created) = (text(&row, 0)?, text(&row, 4)?, text(&row, 5)?);
        let entry = merged
            .entry(shape)
            .or_insert_with(|| (id.clone(), seen.clone(), created.clone()));
        if seen < entry.1 {
            entry.1 = seen;
        }
    }
    drop(rows);
    for ((member, agent, project, kinds, filter, exclude, about, session), (id, seen, created)) in
        merged
    {
        tx.execute(
            "INSERT INTO notify_subscriptions_v28(id, member_id, agent, project_id, kinds,
                last_seen_event_id, created_at, author_filter, exclude_self, about_self, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            vec![
                t(&id),
                t(&member),
                t(&agent),
                ot(project.as_deref()),
                t(&kinds.join(",")),
                t(&seen),
                t(&created),
                ot(filter.as_deref()),
                Value::Integer(exclude),
                Value::Integer(about),
                ot(session.as_deref()),
            ],
        )
        .await
        .map_err(super::engine)?;
    }
    Ok(())
}

// Preflight.

/// Plan the upgrade of the store in `data_dir` without writing to it.
///
/// A store below v27 is copied with `VACUUM INTO` under `staging_root`,
/// migrated there, and planned on the copy; the copy is removed before this
/// returns. A store at v28 or newer plans nothing. A store whose engine lock a
/// serving hub holds is refused with an error for which
/// [`Error::is_locked`](crate::error::Error::is_locked) is true.
pub async fn check(data_dir: &Path, staging_root: &Path) -> Result<Plan> {
    let hub = data_dir.join("hub.db");
    if !hub.is_file() {
        return Ok(Plan::empty(0));
    }
    let map = OwnerMap::load(Some(data_dir))?;
    let db = super::open_engine(&hub).await?;
    preflight(&db, &map, staging_root).await
}

/// Plan the upgrade of an open store, reading only; a pre-v27 store is
/// planned on a staged copy under `staging_root`, removed before returning.
pub(crate) async fn preflight(
    db: &turso::Database,
    map: &OwnerMap,
    staging_root: &Path,
) -> Result<Plan> {
    let version = super::schema_version(db).await?;
    if version == 0 || version > FROM {
        return Ok(Plan::empty(version));
    }
    if version == FROM {
        let conn = super::connect(db)?;
        return Ok(plan_on(&conn, map).await?.plan);
    }
    let staging = staging_root.join(format!(".staging-{}", super::filename_stamp()));
    let planned = staged_plan(db, map, &staging).await;
    if let Err(err) = std::fs::remove_dir_all(&staging)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %staging.display(), error = %err, "could not remove the staged copy");
    }
    let mut plan = planned?;
    plan.from_version = version;
    Ok(plan)
}

/// [`preflight`] with the owner map read from `data_dir`.
pub(crate) async fn preflight_in(
    db: &turso::Database,
    data_dir: &Path,
    staging_root: &Path,
) -> Result<Plan> {
    let map = OwnerMap::load(Some(data_dir))?;
    preflight(db, &map, staging_root).await
}

async fn staged_plan(db: &turso::Database, map: &OwnerMap, staging: &Path) -> Result<Plan> {
    std::fs::create_dir_all(staging)?;
    let copy = staging.join("hub.db");
    vacuum_into(db, &copy).await?;
    let staged = super::open_engine(&copy).await?;
    super::migrate_upto(&staged, None, FROM).await?;
    let conn = super::connect(&staged)?;
    let plan = plan_on(&conn, map).await?.plan;
    drop(conn);
    drop(staged);
    Ok(plan)
}

/// Copy a store with the engine's `VACUUM INTO`.
pub(crate) async fn vacuum_into(db: &turso::Database, dest: &Path) -> Result<()> {
    let target = dest.to_string_lossy().replace('\'', "''");
    let conn = super::connect(db)?;
    conn.execute(&format!("VACUUM INTO '{target}'"), ())
        .await
        .map_err(super::engine)?;
    Ok(())
}

// The merge worker.

/// Where a test stops the merge worker: after a stage's effect, before its
/// commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    AfterMap,
    AfterCopy,
    AfterVerify,
    AfterClose,
    /// Renamed, directories not yet synced, stage not committed.
    AfterArchiveRename,
    AfterArchive,
}

#[derive(Debug, Clone)]
struct MergeRow {
    loser: String,
    survivor: String,
    project: String,
    stage: String,
    loser_present: bool,
    archive_path: String,
}

#[derive(Debug, Clone)]
struct PathRow {
    ns: String,
    src: String,
    kind: String,
    action: String,
    dest: Option<String>,
    sha256: Option<String>,
}

fn fail_at(fail: Option<FailPoint>, point: FailPoint) -> Result<()> {
    if fail == Some(point) {
        return Err(Error::Unavailable(format!("merge stopped at {point:?}")));
    }
    Ok(())
}

fn halted(row: &MergeRow, why: String) -> Error {
    Error::Conflict(format!(
        "merging session {} into {} stopped at stage {}: {why}",
        row.loser, row.survivor, row.stage
    ))
}

/// Drive every merge journal row to `done`. `open` passes `None`.
#[doc(hidden)]
pub async fn run_merges(
    db: &turso::Database,
    data_dir: &Path,
    fail: Option<FailPoint>,
) -> Result<()> {
    let rows = {
        let conn = super::connect(db)?;
        let mut rows = conn
            .query(
                "SELECT loser_session_id, survivor_session_id, project_id, stage, loser_present,
                        archive_path
                   FROM identity_merges WHERE stage <> 'done' ORDER BY loser_session_id",
                (),
            )
            .await
            .map_err(super::engine)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(super::engine)? {
            out.push(MergeRow {
                loser: text(&row, 0)?,
                survivor: text(&row, 1)?,
                project: text(&row, 2)?,
                stage: text(&row, 3)?,
                loser_present: int(&row, 4)? != 0,
                archive_path: text(&row, 5)?,
            });
        }
        out
    };
    for mut row in rows {
        loop {
            let next = match row.stage.as_str() {
                "planned" => map_stage(db, data_dir, &row, fail).await?,
                "mapped" => copy_stage(db, data_dir, &row, fail).await?,
                "copied" => verify_stage(db, data_dir, &row, fail).await?,
                "verified" => close_stage(data_dir, &row, fail).await?,
                "closed" => archive_stage(data_dir, &row, fail).await?,
                "archived" => "done",
                "done" => break,
                other => return Err(halted(&row, format!("unknown stage '{other}'"))),
            };
            // `mapped` and `verified` commit inside their stage's own
            // transaction; the others commit here, once their effect is
            // durable.
            if !matches!(next, "mapped" | "verified") {
                set_stage(db, &row.loser, next).await?;
            }
            row.stage = next.to_string();
        }
    }
    Ok(())
}

async fn set_stage(db: &turso::Database, loser: &str, stage: &str) -> Result<()> {
    let tx = super::begin_write(db).await?;
    tx.execute(
        "UPDATE identity_merges SET stage = ?1, updated_at = ?2 WHERE loser_session_id = ?3",
        vec![t(stage), t(&super::now_rfc3339()), t(loser)],
    )
    .await
    .map_err(super::engine)?;
    tx.commit().await.map_err(super::engine)?;
    Ok(())
}

fn store_of(data_dir: &Path) -> crate::brain::BrainStore {
    crate::brain::BrainStore::for_data_dir(data_dir)
}

fn sha(bytes: &[u8]) -> String {
    crate::brain::version(bytes)
}

/// planned -> mapped: one collision-checked source to destination map,
/// persisted with the conflict root and the stage in one transaction.
async fn map_stage(
    db: &turso::Database,
    data_dir: &Path,
    row: &MergeRow,
    fail: Option<FailPoint>,
) -> Result<&'static str> {
    let store = store_of(data_dir);
    let loser_items = if row.loser_present {
        match store.open_existing(&row.project, &row.loser).await? {
            Some(brain) => brain.inventory().await?,
            None => return Err(halted(row, "loser brain lost".to_string())),
        }
    } else {
        Vec::new()
    };
    let survivor_items = match store.open_existing(&row.project, &row.survivor).await? {
        Some(brain) => brain.inventory().await?,
        None => Vec::new(),
    };
    let (root, paths) = plan_paths(&row.loser, &loser_items, &survivor_items);

    let tx = super::begin_write(db).await?;
    tx.execute(
        "DELETE FROM identity_merge_paths WHERE loser_session_id = ?1",
        vec![t(&row.loser)],
    )
    .await
    .map_err(super::engine)?;
    for path in &paths {
        tx.execute(
            "INSERT INTO identity_merge_paths(loser_session_id, ns, src, kind, action, dest, sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            vec![
                t(&row.loser),
                t(&path.ns),
                t(&path.src),
                t(&path.kind),
                t(&path.action),
                ot(path.dest.as_deref()),
                ot(path.sha256.as_deref()),
            ],
        )
        .await
        .map_err(super::engine)?;
    }
    tx.execute(
        "UPDATE identity_merges SET conflict_root = ?1, stage = 'mapped', updated_at = ?2
          WHERE loser_session_id = ?3",
        vec![t(&root), t(&super::now_rfc3339()), t(&row.loser)],
    )
    .await
    .map_err(super::engine)?;
    fail_at(fail, FailPoint::AfterMap)?;
    tx.commit().await.map_err(super::engine)?;
    Ok("mapped")
}

fn fs_ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = path;
    while let Some((parent, _)) = current.rsplit_once('/') {
        if parent == "/fs" || parent == "/kv" || parent.is_empty() {
            break;
        }
        out.push(parent.to_string());
        current = parent;
    }
    out
}

/// Decide, per loser entry, skip, copy or relocate, and the conflict root.
fn plan_paths(
    loser: &str,
    loser_items: &[crate::brain::MergeItem],
    survivor_items: &[crate::brain::MergeItem],
) -> (String, Vec<PathRow>) {
    use crate::brain::MergeKind;
    let survivor: HashMap<&str, &crate::brain::MergeItem> = survivor_items
        .iter()
        .map(|i| (i.path.as_str(), i))
        .collect();
    let survivor_file_at = |path: &str| {
        survivor
            .get(path)
            .is_some_and(|item| item.kind != MergeKind::Dir)
    };
    let survivor_beneath = |path: &str| {
        let prefix = format!("{path}/");
        survivor_items.iter().any(|i| i.path.starts_with(&prefix))
    };

    // Decide each loser entry, parents before what they hold.
    let mut sorted: Vec<&crate::brain::MergeItem> = loser_items.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut decided: Vec<(&crate::brain::MergeItem, &'static str)> = Vec::new();
    let mut relocated_dirs: Vec<String> = Vec::new();
    for item in sorted {
        let under_relocated = relocated_dirs
            .iter()
            .any(|dir| item.path.starts_with(&format!("{dir}/")));
        let blocked_above = fs_ancestors(&item.path).iter().any(|a| survivor_file_at(a));
        let action = if under_relocated {
            "relocate"
        } else {
            match item.kind {
                MergeKind::Key => match survivor.get(item.path.as_str()) {
                    None => "copy",
                    Some(held) if held.bytes == item.bytes => "skip",
                    Some(_) => "relocate",
                },
                MergeKind::Dir => match survivor.get(item.path.as_str()) {
                    Some(held) if held.kind == MergeKind::Dir => "skip",
                    Some(_) => "relocate",
                    None if blocked_above => "relocate",
                    None => "copy",
                },
                MergeKind::File => match survivor.get(item.path.as_str()) {
                    Some(held) if held.kind == MergeKind::File && held.bytes == item.bytes => {
                        "skip"
                    }
                    Some(_) => "relocate",
                    None if blocked_above || survivor_beneath(&item.path) => "relocate",
                    None => "copy",
                },
            }
        };
        if action == "relocate" && item.kind == MergeKind::Dir {
            relocated_dirs.push(item.path.clone());
        }
        decided.push((item, action));
    }

    // Every path either brain holds and every copy destination: a conflict
    // root keeps clear of all of them.
    let mut taken: HashMap<String, bool> = HashMap::new();
    for item in survivor_items.iter().chain(loser_items.iter()) {
        let is_dir = item.kind == MergeKind::Dir;
        let entry = taken.entry(item.path.clone()).or_insert(is_dir);
        *entry = *entry && is_dir;
    }
    let free = |root: &str| -> bool {
        for ns in ["/kv", "/fs"] {
            let at = format!("{ns}/{root}");
            let beneath = format!("{at}/");
            if taken
                .keys()
                .any(|path| path == &at || path.starts_with(&beneath))
            {
                return false;
            }
            if ns == "/fs"
                && fs_ancestors(&at)
                    .iter()
                    .any(|a| taken.get(a).is_some_and(|is_dir| !is_dir))
            {
                return false;
            }
        }
        true
    };
    let mut n = 1;
    let root = loop {
        let candidate = match n {
            1 => format!("merged/{loser}"),
            2 => format!("merged-{loser}"),
            n => format!("merged-{loser}-{}", n - 1),
        };
        if free(&candidate) {
            break candidate;
        }
        n += 1;
    };

    let rows = decided
        .into_iter()
        .map(|(item, action)| {
            let ns = if item.path.starts_with("/kv/") {
                "kv"
            } else {
                "fs"
            };
            let dest = match action {
                "skip" => None,
                "copy" => Some(item.path.clone()),
                _ => {
                    let rest = item
                        .path
                        .strip_prefix(&format!("/{ns}/"))
                        .unwrap_or(&item.path);
                    Some(format!("/{ns}/{root}/{rest}"))
                }
            };
            PathRow {
                ns: ns.to_string(),
                src: item.path.clone(),
                kind: match item.kind {
                    MergeKind::Key => "key",
                    MergeKind::File => "file",
                    MergeKind::Dir => "dir",
                }
                .to_string(),
                action: action.to_string(),
                sha256: match (action, item.kind) {
                    ("skip", _) | (_, MergeKind::Dir) => None,
                    _ => Some(sha(&item.bytes)),
                },
                dest,
            }
        })
        .collect();
    (root, rows)
}

async fn path_rows(db: &turso::Database, loser: &str) -> Result<Vec<PathRow>> {
    let conn = super::connect(db)?;
    let mut rows = conn
        .query(
            "SELECT ns, src, kind, action, dest, sha256 FROM identity_merge_paths
              WHERE loser_session_id = ?1 ORDER BY src",
            vec![t(loser)],
        )
        .await
        .map_err(super::engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        out.push(PathRow {
            ns: text(&row, 0)?,
            src: text(&row, 1)?,
            kind: text(&row, 2)?,
            action: text(&row, 3)?,
            dest: opt(&row, 4)?,
            sha256: opt(&row, 5)?,
        });
    }
    Ok(out)
}

/// mapped -> copied: write every copy and relocation absent at its
/// destination, never overwriting, then the loser's write-log rows.
async fn copy_stage(
    db: &turso::Database,
    data_dir: &Path,
    row: &MergeRow,
    fail: Option<FailPoint>,
) -> Result<&'static str> {
    let paths = path_rows(db, &row.loser).await?;
    if row.loser_present {
        let store = store_of(data_dir);
        let loser = store
            .open_existing(&row.project, &row.loser)
            .await?
            .ok_or_else(|| halted(row, "loser brain lost".to_string()))?;
        let survivor = store.open(&row.project, &row.survivor).await?;
        for path in &paths {
            let Some(dest) = path.dest.as_deref() else {
                continue;
            };
            if path.kind == "dir" {
                survivor.merge_mkdir(dest).await?;
                continue;
            }
            match survivor.merge_read(dest).await? {
                Some(held) if Some(sha(&held)) == path.sha256 => continue,
                Some(_) => return Err(halted(row, format!("merge destination changed: {dest}"))),
                None => {}
            }
            let bytes = loser
                .merge_read(&path.src)
                .await?
                .ok_or_else(|| halted(row, format!("merge source vanished: {}", path.src)))?;
            survivor.merge_put(dest, &bytes).await?;
        }
        let relocated: HashMap<String, String> = paths
            .iter()
            .filter(|p| p.action == "relocate")
            .filter_map(|p| p.dest.clone().map(|d| (p.src.clone(), d)))
            .collect();
        survivor
            .merge_log_from(&loser, &row.loser, &relocated)
            .await?;
    }
    fail_at(fail, FailPoint::AfterCopy)?;
    Ok("copied")
}

/// copied -> verified: every destination holds its bytes; the survivor's
/// brain search rows are rebuilt from its final paths.
async fn verify_stage(
    db: &turso::Database,
    data_dir: &Path,
    row: &MergeRow,
    fail: Option<FailPoint>,
) -> Result<&'static str> {
    let paths = path_rows(db, &row.loser).await?;
    let store = store_of(data_dir);
    let survivor = store.open_existing(&row.project, &row.survivor).await?;
    if let Some(survivor) = &survivor {
        for path in &paths {
            let (Some(dest), Some(expected)) = (path.dest.as_deref(), path.sha256.as_deref())
            else {
                continue;
            };
            let held = survivor.merge_read(dest).await?;
            if held.as_deref().map(sha).as_deref() != Some(expected) {
                return Err(halted(row, format!("merge destination changed: {dest}")));
            }
        }
    }
    let items = match &survivor {
        Some(brain) => brain.inventory().await?,
        None => Vec::new(),
    };
    let tx = super::begin_write(db).await?;
    crate::brain::shared::rebuild_docs_in_tx(&tx, &row.project, &row.survivor, &items).await?;
    tx.execute(
        "UPDATE identity_merges SET stage = 'verified', updated_at = ?1 WHERE loser_session_id = ?2",
        vec![t(&super::now_rfc3339()), t(&row.loser)],
    )
    .await
    .map_err(super::engine)?;
    fail_at(fail, FailPoint::AfterVerify)?;
    tx.commit().await.map_err(super::engine)?;
    Ok("verified")
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Make a directory's entries durable.
pub(crate) fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

/// verified -> closed: checkpoint and close both files; the loser is one file.
async fn close_stage(
    data_dir: &Path,
    row: &MergeRow,
    fail: Option<FailPoint>,
) -> Result<&'static str> {
    let store = store_of(data_dir);
    let dir = data_dir.join("sessions").join(&row.project);
    if let Some(survivor) = store.open_existing(&row.project, &row.survivor).await? {
        survivor.checkpoint().await?;
    }
    let loser_file = dir.join(format!("{}.db", row.loser));
    if row.loser_present {
        if let Some(loser) = store.open_existing(&row.project, &row.loser).await? {
            loser.checkpoint().await?;
            drop(loser);
            let wal = sidecar(&loser_file, "-wal");
            match std::fs::metadata(&wal) {
                Ok(meta) if meta.len() > 0 => {
                    return Err(halted(
                        row,
                        format!("checkpoint left frames in {}", wal.display()),
                    ));
                }
                Ok(_) => std::fs::remove_file(&wal)?,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
            match std::fs::remove_file(sidecar(&loser_file, "-shm")) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        if dir.is_dir() {
            sync_dir(&dir)?;
        }
    } else if loser_file.exists() {
        return Err(halted(
            row,
            format!(
                "a brain appeared for a loser that had none: {}",
                loser_file.display()
            ),
        ));
    }
    fail_at(fail, FailPoint::AfterClose)?;
    Ok("closed")
}

/// closed -> archived: move the loser file under `merged/`, replaying by the
/// state of source and destination.
async fn archive_stage(
    data_dir: &Path,
    row: &MergeRow,
    fail: Option<FailPoint>,
) -> Result<&'static str> {
    let src_dir = data_dir.join("sessions").join(&row.project);
    let src = src_dir.join(format!("{}.db", row.loser));
    let dst = data_dir.join(&row.archive_path);
    let dst_dir = dst
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| data_dir.to_path_buf());
    match (row.loser_present, src.exists(), dst.exists()) {
        (true, true, false) => {
            crate::config::private_dir(&dst_dir)?;
            sync_dir(&data_dir.join("merged"))?;
            sync_dir(data_dir)?;
            std::fs::rename(&src, &dst)?;
            fail_at(fail, FailPoint::AfterArchiveRename)?;
            sync_dir(&src_dir)?;
            sync_dir(&dst_dir)?;
        }
        (true, false, true) => {
            sync_dir(&src_dir)?;
            sync_dir(&dst_dir)?;
        }
        (true, true, true) => return Err(halted(row, "archive conflict".to_string())),
        (true, false, false) => return Err(halted(row, "loser brain lost".to_string())),
        (false, false, false) => {}
        (false, _, _) => {
            return Err(halted(
                row,
                format!("a file is where none was expected: {}", src.display()),
            ));
        }
    }
    fail_at(fail, FailPoint::AfterArchive)?;
    Ok("archived")
}

/// The archives that follow a session: the merged losers whose survivor it
/// is, as paths under the data directory.
pub(crate) async fn archives_of(conn: &Connection, session_id: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT archive_path FROM identity_merges
              WHERE survivor_session_id = ?1 AND loser_present = 1 AND stage = 'done'",
            vec![t(session_id)],
        )
        .await
        .map_err(super::engine)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(super::engine)? {
        out.push(text(&row, 0)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn labels_place_by_map_reserved_word_id_then_last_slash() {
        let held = members(&["laptop", "claude-code/laptop", "desktop"]);
        let map = OwnerMap::default();
        assert_eq!(
            place("laptop/claude", &held, &map),
            Placement::Participant {
                member: "laptop".into(),
                agent: "claude-code".into()
            }
        );
        assert_eq!(
            place("claude-code/laptop", &held, &map),
            Placement::Participant {
                member: "claude-code/laptop".into(),
                agent: String::new()
            }
        );
        assert_eq!(
            place("claude-code/laptop/x", &held, &map),
            Placement::Participant {
                member: "claude-code/laptop".into(),
                agent: "x".into()
            }
        );
        assert_eq!(
            place("hub", &held, &map),
            Placement::Human {
                origin: Origin::Hub
            }
        );
        assert_eq!(place("stray", &held, &map), Placement::Unplaced);
    }

    #[test]
    fn audiences_read_in_both_shapes() {
        let old = serde_json::json!({"audience": ["laptop/claude"], "audience_sessions": ["s1"]});
        let read = Audience::of(Some(&old));
        assert_eq!(read.labels, vec!["laptop/claude".to_string()]);
        assert!(read.names(Some("s1"), None));
        let new = serde_json::json!({"audience": {"sessions": [], "participants": [{"member": "laptop", "agent": ""}], "labels": []}});
        let key = crate::author::ParticipantKey {
            member: "laptop".into(),
            agent: "opencode".into(),
        };
        assert!(Audience::of(Some(&new)).names(None, Some(&key)));
    }
}
