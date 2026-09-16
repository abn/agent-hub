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
use crate::store::artifacts::{self, NewArtifact};

use super::{HubServer, to_error_data};

#[tool_router(router = artifacts_router, vis = "pub")]
impl HubServer {
    #[tool(description = "Publish an artifact and return its id and version.")]
    async fn artifact_publish(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactPublishParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let actor = self.principal(&context).actor;
        let artifact = artifacts::publish(
            &self.state.db,
            &self.state.data_dir,
            NewArtifact {
                actor: &actor,
                project_id: &params.project_id,
                title: &params.title,
                kind: &params.kind,
                content: params.content.as_bytes(),
                envelope: params.envelope,
            },
        )
        .await
        .map_err(to_error_data)?;

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
        let actor = self.principal(&context).actor;
        let artifact = artifacts::update(
            &self.state.db,
            &self.state.data_dir,
            &actor,
            &params.artifact_id,
            params.content.as_bytes(),
            params.envelope,
        )
        .await
        .map_err(to_error_data)?;

        Ok(CallToolResult::structured(
            json!({ "version": artifact.version }),
        ))
    }

    #[tool(description = "Read an artifact's metadata and its current content.")]
    async fn artifact_get(
        &self,
        Parameters(params): Parameters<ArtifactIdParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let (artifact, bytes) =
            artifacts::get(&self.state.db, &self.state.data_dir, &params.artifact_id)
                .await
                .map_err(to_error_data)?;
        // A protected artifact returns its ciphertext here; decryption is the
        // client's job and never the server's.
        let content = String::from_utf8(bytes).map_err(|_| {
            to_error_data(Error::InvalidArgument(format!(
                "artifact {} content is not UTF-8 text",
                params.artifact_id
            )))
        })?;

        Ok(CallToolResult::structured(json!({
            "title": artifact.title,
            "kind": artifact.kind,
            "version": artifact.version,
            "protected": artifact.protected,
            "content": content,
        })))
    }

    #[tool(description = "List a project's artifacts, most recently updated first.")]
    async fn artifact_list(
        &self,
        Parameters(params): Parameters<ArtifactListParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let listed = artifacts::list(&self.state.db, &params.project_id)
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
    envelope: Option<serde_json::Value>,
}

/// Arguments for `artifact_update`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactUpdateParams {
    artifact_id: String,
    content: String,
    #[serde(default)]
    envelope: Option<serde_json::Value>,
}

/// Arguments for `artifact_get`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactIdParams {
    artifact_id: String,
}

/// Arguments for `artifact_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactListParams {
    project_id: String,
}
