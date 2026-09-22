//! Storage usage: what is on the data volume, by kind and by project, and how
//! much room the volume itself has.

use std::path::Path;

use serde::Serialize;
use turso::{Database, Value};

use crate::error::{Error, Result};

/// Storage used by one project.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectUsage {
    pub project_id: String,
    /// The name the projects list shows, absent when no project row carries
    /// the id.
    pub project_display_name: Option<String>,
    /// The text of the project's feed events: each summary and each payload,
    /// in bytes. The indexes over them are shared and counted in
    /// [`StorageUsage::events_shared_bytes`].
    pub events_bytes: i64,
    pub artifact_bytes: i64,
    pub session_bytes: i64,
    pub kb_bytes: i64,
    /// Ended sessions in the project, and the bytes pruning them frees. A
    /// knowledge base is outside session life and is never counted here.
    pub prunable_sessions: i64,
    pub prunable_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_write: Option<String>,
}

/// Bytes on the data volume by what holds them, which is the stacked bar the
/// storage screen draws.
#[derive(Debug, Clone, Default, Serialize)]
pub struct KindBytes {
    /// The hub store, which holds the feed, the inbox and every index. It
    /// belongs to no single project.
    pub events: i64,
    pub sessions: i64,
    pub artifacts: i64,
    pub knowledge: i64,
}

/// What a prune would reclaim.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Prunable {
    pub sessions: i64,
    pub bytes: i64,
}

/// The node the hub runs on, as three screens print it.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// The host name, from the operating system unless the operator named one.
    pub host: String,
    /// How the hub is reached. The node is the cloud, so it is always local;
    /// the field exists so a surface prints a label rather than inventing one.
    pub mode: &'static str,
}

/// Total storage used, a per-kind and a per-project breakdown, and the volume
/// it all sits on.
#[derive(Debug, Clone, Serialize)]
pub struct StorageUsage {
    /// Bytes the projects hold: artifacts, session brains and knowledge bases.
    pub total_bytes: i64,
    /// Bytes the whole data directory holds, the hub store included.
    pub used_bytes: i64,
    /// Bytes the filesystem holds in total, and how many are free to a
    /// non-root writer. Absent when the volume cannot be measured: a screen
    /// that says it does not know beats one showing 0 of 0.
    pub capacity_bytes: Option<i64>,
    pub free_bytes: Option<i64>,
    /// The data directory, as the operator configured it.
    pub data_path: String,
    pub node: Node,
    pub by_kind: KindBytes,
    /// The part of `by_kind.events` no project row claims: the indexes, the
    /// search corpus, the inbox, the identity tables and the pages the engine
    /// holds free. The rows' `events_bytes` and this add up to
    /// `by_kind.events`.
    pub events_shared_bytes: i64,
    pub prunable: Prunable,
    pub projects: Vec<ProjectUsage>,
}

/// The label a surface prints beside the host name.
const NODE_MODE: &str = "local";

/// What the feed's events weigh, per project, up to the newest event weighed.
///
/// The feed only grows at its head, so a report that holds the last one's
/// weights reads the events above `high_water` and nothing else. Removing
/// events is the one thing that makes the weights wrong, and whoever removes
/// them starts over from [`EventBytes::default`].
#[derive(Debug, Clone, Default)]
pub struct EventBytes {
    /// The newest event already weighed, empty before the first.
    high_water: String,
    by_project: std::collections::HashMap<String, i64>,
}

/// Compute storage usage from artifact sizes, session brain file sizes, and
/// knowledge base file sizes, with the volume the data directory sits on.
pub async fn usage(db: &Database, data_dir: &Path, host: &str) -> Result<StorageUsage> {
    Ok(usage_from(db, data_dir, host, EventBytes::default())
        .await?
        .0)
}

/// The same report, weighing only the events above what `weighed` already
/// holds, and handing back the weights for the next report.
pub async fn usage_from(
    db: &Database,
    data_dir: &Path,
    host: &str,
    mut weighed: EventBytes,
) -> Result<(StorageUsage, EventBytes)> {
    let conn = super::connect(db)?;

    let mut by_project: Vec<ProjectUsage> = Vec::new();
    let ensure = |by_project: &mut Vec<ProjectUsage>, project_id: &str| {
        if !by_project.iter().any(|p| p.project_id == project_id) {
            by_project.push(ProjectUsage {
                project_id: project_id.to_string(),
                project_display_name: None,
                events_bytes: 0,
                artifact_bytes: 0,
                session_bytes: 0,
                kb_bytes: 0,
                prunable_sessions: 0,
                prunable_bytes: 0,
                last_write: None,
            });
        }
    };

    // Every version keeps its blob on disk (only delete removes the tree), so
    // the report sums the version rows rather than just the current pointer.
    let mut artifacts = conn
        .query(
            "SELECT a.project_id, COALESCE(SUM(v.size_bytes), 0)
             FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id
             GROUP BY a.project_id",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = artifacts.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let bytes = integer(row.get_value(1).map_err(engine)?);
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.artifact_bytes = bytes;
        }
    }
    drop(artifacts);

    // One pass over the live sessions serves both the per-project total and
    // what an ended session would give back, so the prunable hint costs no
    // second walk.
    let mut prunable = Prunable::default();
    let mut sessions = conn
        .query(
            "SELECT project_id, brain_path, status FROM sessions WHERE deleted_at IS NULL",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = sessions.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let brain_path = text(row.get_value(1).map_err(engine)?);
        let ended = text(row.get_value(2).map_err(engine)?) == "ended";
        let bytes = crate::brain::file_bytes(&data_dir.join(&brain_path));
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.session_bytes += bytes;
            if ended {
                entry.prunable_sessions += 1;
                entry.prunable_bytes += bytes;
            }
        }
        if ended {
            prunable.sessions += 1;
            prunable.bytes += bytes;
        }
    }
    drop(sessions);

    // Each project's events are weighed by the text the agents wrote. The
    // cast makes the length a byte count rather than a character count. Only
    // the events above the last report's newest are read, by the primary key,
    // so a report costs what was appended since and not the whole feed. The
    // first report has nothing to stand on and reads the table in order, which
    // is far cheaper than walking all of it through the key. Either way it is
    // one statement, so the newest id it meets is exactly where it stopped.
    const WEIGH: &str = "SELECT project_id, id,
                LENGTH(CAST(summary AS BLOB)) + LENGTH(CAST(COALESCE(payload, '') AS BLOB))
         FROM events";
    let mut events = if weighed.high_water.is_empty() {
        conn.query(WEIGH, ()).await
    } else {
        conn.query(
            &format!("{WEIGH} WHERE id > ?1"),
            vec![Value::Text(weighed.high_water.clone())],
        )
        .await
    }
    .map_err(engine)?;
    while let Some(row) = events.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let id = text(row.get_value(1).map_err(engine)?);
        *weighed.by_project.entry(project_id).or_insert(0) +=
            integer(row.get_value(2).map_err(engine)?);
        if id > weighed.high_water {
            weighed.high_water = id;
        }
    }
    drop(events);
    for (project_id, bytes) in &weighed.by_project {
        ensure(&mut by_project, project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == *project_id) {
            entry.events_bytes = *bytes;
        }
    }

    // The most recent event timestamp per project.
    let mut last_writes = conn
        .query(
            "SELECT project_id, MAX(created_at) FROM events GROUP BY project_id",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = last_writes.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let at = text(row.get_value(1).map_err(engine)?);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.last_write = Some(at);
        }
    }
    drop(last_writes);

    // Any session activity newer than the latest event timestamp.
    let mut session_activity = conn
        .query(
            "SELECT project_id, MAX(last_activity) FROM sessions WHERE deleted_at IS NULL GROUP BY project_id",
            (),
        )
        .await
        .map_err(engine)?;
    while let Some(row) = session_activity.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let at = text(row.get_value(1).map_err(engine)?);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id)
            && entry.last_write.as_ref().is_none_or(|prev| &at > prev)
        {
            entry.last_write = Some(at);
        }
    }
    drop(session_activity);

    // A knowledge base is never pruned, so it appears here and never in what
    // the human can reclaim. Every project is listed, one that holds nothing
    // with zeros: a project a prune has just emptied keeps its row, so the
    // screen has somewhere to show the undo.
    // The same read names every row, so the names cost no query of their own.
    let mut names = std::collections::HashMap::new();
    let mut projects = conn
        .query("SELECT id, display_name FROM projects", ())
        .await
        .map_err(engine)?;
    while let Some(row) = projects.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        names.insert(project_id.clone(), text(row.get_value(1).map_err(engine)?));
        let bytes = knowledge_bytes(data_dir, &project_id);
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.kb_bytes = bytes;
        }
    }
    drop(projects);
    // Weights can be held across a project's going. A project with no row of
    // its own is gone: it is not listed for the bytes its events once had, and
    // its weight is let go so a project made later with the same id starts
    // from nothing.
    weighed
        .by_project
        .retain(|project_id, _| names.contains_key(project_id));
    by_project.retain(|entry| {
        names.contains_key(&entry.project_id)
            || entry.session_bytes > 0
            || entry.artifact_bytes > 0
            || entry.kb_bytes > 0
    });
    for entry in &mut by_project {
        entry.project_display_name = names.get(&entry.project_id).cloned();
        if !names.contains_key(&entry.project_id) {
            entry.events_bytes = 0;
        }
    }

    by_project.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    let by_kind = KindBytes {
        events: crate::brain::file_bytes(&data_dir.join("hub.db")),
        sessions: by_project.iter().map(|p| p.session_bytes).sum(),
        artifacts: by_project.iter().map(|p| p.artifact_bytes).sum(),
        knowledge: by_project.iter().map(|p| p.kb_bytes).sum(),
    };
    let total_bytes = by_kind.sessions + by_kind.artifacts + by_kind.knowledge;
    // The file always outweighs the text in it. The floor only guards the
    // instant between the two reads.
    let events_text: i64 = by_project.iter().map(|p| p.events_bytes).sum();
    let events_shared_bytes = (by_kind.events - events_text).max(0);
    let volume = volume(data_dir);
    let usage = StorageUsage {
        total_bytes,
        used_bytes: total_bytes + by_kind.events,
        capacity_bytes: volume.map(|(capacity, _)| capacity),
        free_bytes: volume.map(|(_, free)| free),
        data_path: data_dir.display().to_string(),
        node: Node {
            host: host.to_string(),
            mode: NODE_MODE,
        },
        by_kind,
        events_shared_bytes,
        prunable,
        projects: by_project,
    };
    Ok((usage, weighed))
}

/// The volume's capacity and the space free to a writer that is not root.
///
/// One syscall. A failure is reported as "not measured" rather than as zero:
/// the data directory can be gone or unreadable, and a storage screen showing
/// 0 of 0 would be a number that is real-looking and wrong.
fn volume(data_dir: &Path) -> Option<(i64, i64)> {
    match rustix::fs::statvfs(data_dir) {
        Ok(stat) => {
            let block = i64::try_from(stat.f_frsize).ok()?;
            Some((
                i64::try_from(stat.f_blocks).ok()? * block,
                i64::try_from(stat.f_bavail).ok()? * block,
            ))
        }
        Err(err) => {
            tracing::warn!(
                path = %data_dir.display(),
                error = %err,
                "could not measure the data volume"
            );
            None
        }
    }
}

/// The node's host name, unless the operator named one.
///
/// A container's host name is a generated hex string, which tells the human
/// nothing, so `HUB_NODE_NAME` overrides it.
pub fn host_name(configured: Option<&str>) -> String {
    if let Some(name) = configured {
        return name.to_string();
    }
    rustix::system::uname()
        .nodename()
        .to_str()
        .map(str::to_string)
        .unwrap_or_default()
}

/// The bytes a project's knowledge base holds, including what the engine
/// keeps in the write-ahead log beside it.
fn knowledge_bytes(data_dir: &Path, project_id: &str) -> i64 {
    let file = crate::brain::knowledge_dir(data_dir)
        .join(project_id)
        .join(format!("{}.db", crate::brain::KNOWLEDGE_FILE));
    crate::brain::file_bytes(&file)
}

fn text(value: Value) -> String {
    match value {
        Value::Text(value) => value,
        _ => String::new(),
    }
}

fn integer(value: Value) -> i64 {
    match value {
        Value::Integer(value) => value,
        _ => 0,
    }
}

fn engine(err: turso::Error) -> Error {
    Error::Engine(err.to_string())
}

/// What one walk of a project's knowledge base derives.
///
/// Held per project, and served only for the generation it was computed at:
/// every write to a knowledge base, from either surface, bumps the generation.
#[derive(Debug, Clone)]
pub struct KbCacheEntry {
    pub generation: u64,
    pub at: std::time::Instant,
    /// When the walk ran, which is what a served entry reports, never the
    /// time of the request that was handed it.
    pub checked_at: String,
    pub backlink_graph: crate::okf::BacklinkGraph,
    /// Findings, each with the row of the page it names.
    pub lint_findings: Vec<serde_json::Value>,
    pub stats: serde_json::Value,
    /// Every page and directory, by path.
    pub meta_pages: Vec<serde_json::Value>,
}

/// The memo over the numbers that cost a syscall or a walk.
///
/// SQL counts are not cached: they are indexed, they are cheap, and a cached
/// count is the classic source of a number that is real but wrong. What is
/// cached is the volume, the brain file sizes and the prunable total.
///
/// Invalidation is explicit rather than a subscription. Every write bumps a
/// generation counter, and a read serves the memo only when the generation
/// still matches and the entry is fresh, so there is no background task and no
/// subscriber to leak.
#[derive(Debug, Default)]
pub struct StatsCache {
    entry: std::sync::Mutex<Option<Cached>>,
    events: std::sync::Mutex<Option<(std::time::Instant, EventBytes)>>,
    /// How many times the weights have been forgotten. A report notes it
    /// before it weighs and keeps what it weighed only if nothing was forgotten
    /// in between.
    forgets: std::sync::atomic::AtomicU64,
    kb_cache: std::sync::Mutex<std::collections::HashMap<String, KbCacheEntry>>,
}

#[derive(Debug)]
struct Cached {
    generation: u64,
    at: std::time::Instant,
    usage: StorageUsage,
}

/// How long a memo stays fresh when nothing has been written.
const STATS_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the event weights are added to before the feed is weighed afresh.
///
/// Both paths that remove events drop the weights themselves. This bounds how
/// long a number could stay wrong if a later one forgets to.
const EVENT_BYTES_TTL: std::time::Duration = std::time::Duration::from_secs(600);

impl StatsCache {
    /// A memo with nothing in it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve fresh cached knowledge base derived data if generation matches.
    pub fn get_kb(&self, project_id: &str, generation: u64) -> Option<KbCacheEntry> {
        let cache = self.kb_cache.lock().ok()?;
        let entry = cache.get(project_id)?;
        if entry.generation == generation && entry.at.elapsed() < STATS_TTL {
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Update cached knowledge base derived data.
    pub fn set_kb(&self, project_id: &str, mut entry: KbCacheEntry) {
        entry.at = std::time::Instant::now();
        if let Ok(mut cache) = self.kb_cache.lock() {
            cache.insert(project_id.to_string(), entry);
        }
    }

    /// The usage report, computed unless a fresh one for this generation is
    /// already held.
    pub async fn usage(
        &self,
        db: &Database,
        data_dir: &Path,
        host: &str,
        generation: u64,
    ) -> Result<StorageUsage> {
        if let Some(cached) = self.fresh(generation) {
            return Ok(cached);
        }
        let started = self.weighing();
        let weighed = self
            .events
            .lock()
            .ok()
            .and_then(|mut held| held.take())
            .filter(|(since, _)| since.elapsed() < EVENT_BYTES_TTL);
        let since = weighed
            .as_ref()
            .map_or_else(std::time::Instant::now, |(since, _)| *since);
        let (usage, weighed) = usage_from(
            db,
            data_dir,
            host,
            weighed.map(|(_, weighed)| weighed).unwrap_or_default(),
        )
        .await?;
        self.keep_weights_since(started, since, weighed);
        if let Ok(mut entry) = self.entry.lock() {
            *entry = Some(Cached {
                generation,
                at: std::time::Instant::now(),
                usage: usage.clone(),
            });
        }
        Ok(usage)
    }

    /// Drop the event weights, so the next report weighs the whole feed.
    ///
    /// Called by whatever removes events: the weights only ever add.
    pub fn forget_events(&self) {
        if let Ok(mut held) = self.events.lock() {
            // Counted under the same lock the weights are kept under, so a
            // report either sees the count move or has its weights dropped.
            self.forgets
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *held = None;
        }
    }

    /// What a report notes before it weighs: how often the weights have been
    /// forgotten so far.
    pub fn weighing(&self) -> u64 {
        self.forgets.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Keep what a report weighed, unless the weights were forgotten while it
    /// was weighing. A report can be in flight across a project delete or a
    /// committed prune, and what it weighed was weighed before the events
    /// went: putting that back would undo the forgetting.
    pub fn keep_weights(&self, started: u64, weighed: EventBytes) -> bool {
        self.keep_weights_since(started, std::time::Instant::now(), weighed)
    }

    fn keep_weights_since(
        &self,
        started: u64,
        since: std::time::Instant,
        weighed: EventBytes,
    ) -> bool {
        let Ok(mut held) = self.events.lock() else {
            return false;
        };
        if self.weighing() != started {
            return false;
        }
        *held = Some((since, weighed));
        true
    }

    fn fresh(&self, generation: u64) -> Option<StorageUsage> {
        let entry = self.entry.lock().ok()?;
        let cached = entry.as_ref()?;
        (cached.generation == generation && cached.at.elapsed() < STATS_TTL)
            .then(|| cached.usage.clone())
    }
}
