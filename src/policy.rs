//! Authorization policy: what a principal may read and write.
//!
//! One seam the tools call before touching a project. The human admin reaches
//! everything. A trusted agent reads everything and writes human-owned
//! (shared) projects and its own, but not another agent's personal space. An
//! untrusted agent reaches only its own space and the projects it is granted,
//! at the granted level.

use turso::{Database, Value};

use crate::error::{Error, Result};
use crate::principal::{Principal, Trust};
use crate::store::{engine, projects};

/// The access a caller asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Read a project's feed, artifacts, inbox, or search results.
    Read,
    /// Write to a project.
    Write,
}

/// Which projects a principal may reach at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    /// Every project: the admin and trusted agents.
    All,
    /// Only these projects.
    Only(Vec<String>),
}

impl Visibility {
    /// The project set for a store filter, or `None` for every project.
    pub fn as_filter(&self) -> Option<&[String]> {
        match self {
            Visibility::All => None,
            Visibility::Only(ids) => Some(ids),
        }
    }
}

/// Reject a caller that may not reach a project.
pub async fn authorize(
    db: &Database,
    principal: &Principal,
    project_id: &str,
    access: Access,
) -> Result<()> {
    if principal.is_admin {
        return Ok(());
    }
    let project = projects::get(db, project_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("project {project_id} not found")))?;

    let allowed = match principal.trust {
        Trust::Trusted => match access {
            Access::Read => true,
            // Trusted writes shared projects and its own, never another
            // agent's personal space.
            Access::Write => {
                project.owner_agent.is_none()
                    || owned_by(&project.owner_agent, principal.agent_id.as_deref())
            }
        },
        // Grants open a project to an untrusted agent. A trusted agent already
        // reaches shared projects, so grants are the untrusted path.
        Trust::Untrusted => {
            if owned_by(&project.owner_agent, principal.agent_id.as_deref()) {
                true
            } else {
                match principal.agent_id.as_deref() {
                    Some(agent_id) => {
                        match grant_access(db, agent_id, project_id).await?.as_deref() {
                            Some("write") => true,
                            Some("read") => access == Access::Read,
                            _ => false,
                        }
                    }
                    None => false,
                }
            }
        }
    };

    if allowed {
        Ok(())
    } else {
        Err(Error::Forbidden(format!(
            "{} may not {} project {project_id}",
            principal.actor,
            match access {
                Access::Read => "read",
                Access::Write => "write",
            }
        )))
    }
}

/// The projects a principal may reach. The admin and trusted agents see every
/// project; an untrusted agent sees its personal space and its grants.
pub async fn visibility(db: &Database, principal: &Principal) -> Result<Visibility> {
    if principal.is_admin || principal.trust == Trust::Trusted {
        return Ok(Visibility::All);
    }
    let Some(agent_id) = principal.agent_id.as_deref() else {
        return Ok(Visibility::Only(Vec::new()));
    };
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT id FROM projects WHERE owner_agent = ?1
             UNION
             SELECT project_id FROM grants WHERE agent_id = ?1
             ORDER BY 1",
            [agent_id],
        )
        .await
        .map_err(engine)?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(engine)? {
            ids.push(id);
        }
    }
    Ok(Visibility::Only(ids))
}

/// The access level of an agent's grant on a project, if any.
async fn grant_access(db: &Database, agent_id: &str, project_id: &str) -> Result<Option<String>> {
    let conn = db.connect().map_err(engine)?;
    let mut rows = conn
        .query(
            "SELECT access FROM grants WHERE agent_id = ?1 AND project_id = ?2",
            [agent_id, project_id],
        )
        .await
        .map_err(engine)?;
    match rows.next().await.map_err(engine)? {
        Some(row) => match row.get_value(0).map_err(engine)? {
            Value::Text(access) => Ok(Some(access)),
            other => Err(Error::Engine(format!(
                "expected text in a grant column, found {other:?}"
            ))),
        },
        None => Ok(None),
    }
}

/// Whether a project owner column matches a caller's agent id. A `NULL` owner
/// is a shared, human-owned project, not an agent's own.
fn owned_by(owner: &Option<String>, agent_id: Option<&str>) -> bool {
    match (owner.as_deref(), agent_id) {
        (Some(owner), Some(agent_id)) => owner == agent_id,
        _ => false,
    }
}
