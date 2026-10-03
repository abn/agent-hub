//! Artifact tools: publish, update, read, and list.
//!
//! A protected artifact arrives as ciphertext plus an encryption envelope; the
//! server stores both and never sees the plaintext. The actor on every write is
//! the resolved principal, never client input.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::schemars::{self, JsonSchema};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use crate::error::Error;
use crate::policy::{self, Access};
use crate::store::artifacts::{self, NewArtifact, UpdateOptions};

use super::{HubServer, to_error_data};

#[tool_router(router = artifacts_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Publish an artifact and return its id and version.")]
    async fn artifact_publish(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactPublishParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        policy::authorize(
            &self.state.db,
            &principal,
            &params.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let session_id = self.session_in(&principal, &params.project_id).await;
        let artifact = artifacts::publish_for_principal_capped(
            &self.state.db,
            &self.state.data_dir,
            Some(&principal),
            self.state.config.events_per_project.per_project,
            NewArtifact {
                actor: &principal.actor,
                project_id: &params.project_id,
                title: &params.title,
                description: &params.description,
                favicon: &params.favicon,
                label: params.label.as_deref(),
                kind: &params.kind,
                content: params.content.as_bytes(),
                envelope: params.envelope,
                session_id: session_id.as_deref(),
            },
            params.idempotency_key.as_deref(),
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(json!({
            "artifact_id": artifact.id,
            "version": artifact.version,
        })))
    }

    #[tool(description = "Publish a new version of an existing artifact.")]
    async fn artifact_update(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactUpdateParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &existing.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let session_id = self.session_in(&principal, &existing.project_id).await;
        let artifact = artifacts::update_for_principal_capped(
            &self.state.db,
            &self.state.data_dir,
            Some(&principal),
            self.state.config.events_per_project.per_project,
            &principal.actor,
            &params.artifact_id,
            params.content.as_bytes(),
            match params.envelope {
                None => artifacts::EnvelopeUpdate::Keep,
                Some(None) => artifacts::EnvelopeUpdate::Clear,
                Some(Some(envelope)) => artifacts::EnvelopeUpdate::Set(envelope),
            },
            UpdateOptions {
                base_version: params.base_version,
                force: params.force,
                label: params.label.as_ref().map(|opt| opt.as_deref()),
                session_id: session_id.as_deref(),
            },
            params.idempotency_key.as_deref(),
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(
            json!({ "version": artifact.version }),
        ))
    }

    #[tool(description = "Read an artifact's metadata and its content, optionally at a version.")]
    async fn artifact_get(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactIdParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &existing.project_id,
            Access::Read,
        )
        .await
        .map_err(to_error_data)?;
        let (artifact, bytes) = match params.version {
            Some(version) => artifacts::get_at_version(
                &self.state.db,
                &self.state.data_dir,
                &params.artifact_id,
                version,
            )
            .await
            .map_err(to_error_data)?,
            None => {
                let bytes = artifacts::read_blob(&self.state.data_dir, &existing)
                    .await
                    .map_err(to_error_data)?;
                (existing, bytes)
            }
        };
        // A protected artifact returns its ciphertext here; decryption is the
        // client's job and never the server's.
        let content = String::from_utf8(bytes).map_err(|_| {
            to_error_data(Error::InvalidArgument(format!(
                "artifact {} content is not UTF-8 text",
                params.artifact_id
            )))
        })?;

        Ok(CallToolResult::structured(json!({
            "actor": artifact.actor,
            "title": artifact.title,
            "description": artifact.description,
            "favicon": artifact.favicon,
            "label": artifact.label,
            "kind": artifact.kind,
            "version": artifact.version,
            "protected": artifact.protected,
            "content": content,
            "comments_count": artifact.comments_count,
            "comments_open": artifact.comments_open,
        })))
    }

    #[tool(description = "List an artifact's versions, oldest first.")]
    async fn artifact_versions(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactVersionsParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let artifact = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &artifact.project_id,
            Access::Read,
        )
        .await
        .map_err(to_error_data)?;
        let versions = artifacts::list_versions(&self.state.db, &params.artifact_id)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "versions": versions })))
    }

    #[tool(description = "Delete an artifact and its history.")]
    async fn artifact_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactDeleteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(
            &self.state.db,
            &principal,
            &existing.project_id,
            Access::Write,
        )
        .await
        .map_err(to_error_data)?;
        let deleted = artifacts::delete(
            &self.state.db,
            &self.state.data_dir,
            &principal.actor,
            &params.artifact_id,
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(
            json!({ "artifact_id": deleted.id }),
        ))
    }

    #[tool(description = "List a project's artifacts, most recently updated first.")]
    async fn artifact_list(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        policy::authorize(&self.state.db, &principal, &params.project_id, Access::Read)
            .await
            .map_err(to_error_data)?;
        let session_id = params.session.as_deref().or(params.session_id.as_deref());
        let listed = artifacts::list_with_session(&self.state.db, &params.project_id, session_id)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "artifacts": listed })))
    }
}

/// Arguments for `artifact_publish`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactPublishParams {
    project_id: String,
    title: String,
    kind: String,
    content: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    favicon: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    envelope: Option<serde_json::Value>,
    /// Any supplied session_id is ignored to prevent forgery; lineage comes from principal.
    ///
    /// Deserialised for compatibility, never advertised: a field the tool
    /// discards is not part of its contract.
    #[serde(default)]
    #[allow(dead_code)]
    #[schemars(skip)]
    session_id: Option<String>,
    /// Any supplied actor is ignored to prevent forgery; lineage comes from principal.
    #[serde(default)]
    #[allow(dead_code)]
    #[schemars(skip)]
    actor: Option<String>,
    /// Optional idempotency key, so a retried publish returns the original.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Arguments for `artifact_update`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactUpdateParams {
    artifact_id: String,
    content: String,
    /// Absent keeps the artifact's current envelope, an envelope replaces it,
    /// and an explicit null publishes this version in the clear. The three are
    /// distinct, so an agent that says nothing never has its ciphertext stored
    /// as if it were plaintext.
    #[serde(default, deserialize_with = "present_or_absent")]
    #[schemars(with = "Option<serde_json::Value>")]
    envelope: Option<Option<serde_json::Value>>,
    #[serde(default)]
    base_version: Option<i64>,
    #[serde(default)]
    force: bool,
    /// Absent keeps the current label, an explicit null or empty string clears
    /// it, and a string sets a new label.
    #[serde(default, deserialize_with = "present_or_absent")]
    #[schemars(with = "Option<String>")]
    label: Option<Option<String>>,
    /// Optional idempotency key, so a retried update returns the original.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Read a field that may be absent or explicitly null into a nested option, so
/// the two can be told apart. Serde's own default handles the absent case.
fn present_or_absent<'de, T, D>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Arguments for `artifact_get`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactIdParams {
    artifact_id: String,
    #[serde(default)]
    version: Option<i64>,
}

/// Arguments for `artifact_versions`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactVersionsParams {
    artifact_id: String,
}

/// Arguments for `artifact_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactDeleteParams {
    artifact_id: String,
}

/// Arguments for `artifact_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactListParams {
    project_id: String,
    #[serde(default)]
    session: Option<String>,
    /// Duplicate spelling kept for compatibility; `session` is the advertised
    /// field, so this one is accepted but not shown.
    #[serde(default)]
    #[schemars(skip)]
    session_id: Option<String>,
}
