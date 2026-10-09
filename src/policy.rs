//! Authorization policy: what a principal may read and write.
//!
//! One seam the tools call before touching a project. The human admin reaches
//! everything. An authenticated agent reads and writes every project that is not
//! confidential, but not another agent's personal space. A confidential project
//! requires an explicit grant.

use turso::{Database, Value};

use crate::error::{Error, ErrorCode, Result};
use crate::principal::Principal;
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
    /// Every project: the admin.
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
    // A non-admin never learns whether a project exists: a missing one and a
    // denied one both return the same value, so neither the code nor the
    // message is an existence oracle. The detail goes to the log.
    let project = projects::get(db, project_id).await?.ok_or_else(|| {
        tracing::debug!(actor = %principal.actor, project = %project_id, "project not found");
        denied()
    })?;

    // Ownership rule: writing to another agent's personal space is never allowed.
    if access == Access::Write
        && project.owner_agent.is_some()
        && !owned_by(&project.owner_agent, principal.agent_id.as_deref())
    {
        tracing::debug!(
            actor = %principal.actor,
            project = %project_id,
            ?access,
            "cannot write another agent's personal space"
        );
        return Err(denied());
    }

    // Confidentiality rule: access to a confidential project requires an explicit grant.
    if project.confidential {
        let has_grant = match principal.agent_id.as_deref() {
            Some(agent_id) => crate::store::identity::has_grant(db, agent_id, project_id).await?,
            None => false,
        };
        if !has_grant {
            tracing::debug!(
                actor = %principal.actor,
                project = %project_id,
                ?access,
                "confidential project without grant"
            );
            return Err(denied());
        }
    }

    Ok(())
}

/// Reject a caller that may not reach a project, evaluated inside an active write transaction.
///
/// Guards against TOCTOU races where a grant is revoked, confidentiality changed, or
/// project deleted between pre-transaction authorization and write commit.
pub(crate) async fn authorize_in_tx(
    tx: &crate::store::WriteTx,
    principal: &Principal,
    project_id: &str,
    access: Access,
) -> Result<()> {
    let mut rows = tx
        .query(
            "SELECT id, owner_agent, confidential, status FROM projects WHERE id = ?1",
            vec![Value::Text(project_id.to_string())],
        )
        .await
        .map_err(engine)?;
    let row = match rows.next().await.map_err(engine)? {
        Some(r) => r,
        None => {
            tracing::debug!(actor = %principal.actor, project = %project_id, "project not found in tx");
            return Err(if principal.is_admin {
                Error::NotFound(format!("project {project_id} not found"))
            } else {
                denied()
            });
        }
    };
    let status: String = match row.get_value(3).map_err(engine)? {
        Value::Text(s) => s,
        _ => "active".to_string(),
    };
    if status != "active" {
        tracing::debug!(actor = %principal.actor, project = %project_id, "project not active in tx");
        return Err(if principal.is_admin {
            Error::NotFound(format!("project {project_id} not found"))
        } else {
            denied()
        });
    }

    if principal.is_admin {
        return Ok(());
    }

    let owner_agent = match row.get_value(1).map_err(engine)? {
        Value::Text(val) => Some(val),
        _ => None,
    };
    if access == Access::Write
        && owner_agent.is_some()
        && !owned_by(&owner_agent, principal.agent_id.as_deref())
    {
        tracing::debug!(
            actor = %principal.actor,
            project = %project_id,
            ?access,
            "cannot write another agent's personal space in tx"
        );
        return Err(denied());
    }

    let confidential = match row.get_value(2).map_err(engine)? {
        Value::Integer(val) => val != 0,
        _ => false,
    };
    if confidential {
        let has_grant = match principal.agent_id.as_deref() {
            Some(agent_id) => {
                let mut grant_rows = tx
                    .query(
                        "SELECT 1 FROM grants WHERE agent_id = ?1 AND project_id = ?2",
                        vec![
                            Value::Text(agent_id.to_string()),
                            Value::Text(project_id.to_string()),
                        ],
                    )
                    .await
                    .map_err(engine)?;
                grant_rows.next().await.map_err(engine)?.is_some()
            }
            None => false,
        };
        if !has_grant {
            tracing::debug!(
                actor = %principal.actor,
                project = %project_id,
                ?access,
                "confidential project without grant in tx"
            );
            return Err(denied());
        }
    }

    Ok(())
}

/// The one denial a non-admin caller sees, shared by every non-admin refusal
/// so that a missing resource and a denied one are indistinguishable.
fn denied() -> Error {
    Error::Forbidden("not found or not permitted".to_string())
}

/// Collapse a missing resource to `Forbidden` for a non-admin caller, so an
/// agent cannot tell "does not exist" from "not allowed". The admin, who can
/// reach everything, keeps the precise error.
pub fn conceal(principal: &Principal, error: Error) -> Error {
    if principal.is_admin || error.code() != ErrorCode::NotFound {
        return error;
    }
    denied()
}

/// The projects a principal may reach. The admin sees every project;
/// an agent sees every non-confidential project plus any confidential project
/// it has been granted.
pub async fn visibility(db: &Database, principal: &Principal) -> Result<Visibility> {
    if principal.is_admin {
        return Ok(Visibility::All);
    }
    let conn = crate::store::connect(db)?;
    let mut rows = match principal.agent_id.as_deref() {
        Some(agent_id) => conn
            .query(
                "SELECT id FROM projects WHERE confidential = 0
                 UNION
                 SELECT project_id FROM grants WHERE agent_id = ?1
                 ORDER BY 1",
                [agent_id],
            )
            .await
            .map_err(engine)?,
        None => conn
            .query(
                "SELECT id FROM projects WHERE confidential = 0 ORDER BY 1",
                (),
            )
            .await
            .map_err(engine)?,
    };
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(engine)? {
        if let Value::Text(id) = row.get_value(0).map_err(engine)? {
            ids.push(id);
        }
    }
    Ok(Visibility::Only(ids))
}

/// Whether a project owner column matches a caller's agent id. A `NULL` owner
/// is a shared, human-owned project, not an agent's own.
fn owned_by(owner: &Option<String>, agent_id: Option<&str>) -> bool {
    match (owner.as_deref(), agent_id) {
        (Some(owner), Some(agent_id)) => owner == agent_id,
        _ => false,
    }
}
