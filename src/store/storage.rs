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
    pub artifact_bytes: i64,
    pub session_bytes: i64,
    pub kb_bytes: i64,
    /// Ended sessions in the project, and the bytes pruning them frees. A
    /// knowledge base is outside session life and is never counted here.
    pub prunable_sessions: i64,
    pub prunable_bytes: i64,
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
    pub prunable: Prunable,
    pub projects: Vec<ProjectUsage>,
}

/// The label a surface prints beside the host name.
const NODE_MODE: &str = "local";

/// Compute storage usage from artifact sizes, session brain file sizes, and
/// knowledge base file sizes, with the volume the data directory sits on.
pub async fn usage(db: &Database, data_dir: &Path, host: &str) -> Result<StorageUsage> {
    let conn = super::connect(db)?;

    let mut by_project: Vec<ProjectUsage> = Vec::new();
    let ensure = |by_project: &mut Vec<ProjectUsage>, project_id: &str| {
        if !by_project.iter().any(|p| p.project_id == project_id) {
            by_project.push(ProjectUsage {
                project_id: project_id.to_string(),
                artifact_bytes: 0,
                session_bytes: 0,
                kb_bytes: 0,
                prunable_sessions: 0,
                prunable_bytes: 0,
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

    // A knowledge base is never pruned, so it appears here and never in what
    // the human can reclaim. Only a project that has one is listed, so the
    // report still names the projects that hold something.
    let mut projects = conn
        .query("SELECT id FROM projects", ())
        .await
        .map_err(engine)?;
    while let Some(row) = projects.next().await.map_err(engine)? {
        let project_id = text(row.get_value(0).map_err(engine)?);
        let bytes = knowledge_bytes(data_dir, &project_id);
        if bytes == 0 {
            continue;
        }
        ensure(&mut by_project, &project_id);
        if let Some(entry) = by_project.iter_mut().find(|p| p.project_id == project_id) {
            entry.kb_bytes = bytes;
        }
    }
    drop(projects);

    by_project.sort_by(|a, b| a.project_id.cmp(&b.project_id));
    let by_kind = KindBytes {
        events: crate::brain::file_bytes(&data_dir.join("hub.db")),
        sessions: by_project.iter().map(|p| p.session_bytes).sum(),
        artifacts: by_project.iter().map(|p| p.artifact_bytes).sum(),
        knowledge: by_project.iter().map(|p| p.kb_bytes).sum(),
    };
    let total_bytes = by_kind.sessions + by_kind.artifacts + by_kind.knowledge;
    let volume = volume(data_dir);
    Ok(StorageUsage {
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
        prunable,
        projects: by_project,
    })
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
}

#[derive(Debug)]
struct Cached {
    generation: u64,
    at: std::time::Instant,
    usage: StorageUsage,
}

/// How long a memo stays fresh when nothing has been written.
const STATS_TTL: std::time::Duration = std::time::Duration::from_secs(10);

impl StatsCache {
    /// A memo with nothing in it.
    pub fn new() -> Self {
        Self::default()
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
        let usage = usage(db, data_dir, host).await?;
        if let Ok(mut entry) = self.entry.lock() {
            *entry = Some(Cached {
                generation,
                at: std::time::Instant::now(),
                usage: usage.clone(),
            });
        }
        Ok(usage)
    }

    fn fresh(&self, generation: u64) -> Option<StorageUsage> {
        let entry = self.entry.lock().ok()?;
        let cached = entry.as_ref()?;
        (cached.generation == generation && cached.at.elapsed() < STATS_TTL)
            .then(|| cached.usage.clone())
    }
}
