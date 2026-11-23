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
use crate::store::artifacts::{self, EnvelopeUpdate, LiveOptions, NewArtifact, UpdateOptions};

use super::brain::with_session;
use super::feed::author_filter;
use super::session_arg::{Mode, SessionArg};
use super::{HubServer, to_error_data};

/// The request headers for a tool call, or an empty set on a transport with no
/// HTTP request (stdio, where the local admin is the caller).
fn request_headers(context: &RequestContext<RoleServer>) -> axum::http::HeaderMap {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .map(|parts| parts.headers.clone())
        .unwrap_or_default()
}

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
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                Some(&params.project_id),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let session_id = resolved.session_id().map(str::to_string);
        // The write holds its session's gate shared, so ending or pruning the
        // session waits for it and a write that lost the race is refused.
        let _gate = self.hold_gate(session_id.as_deref()).await;
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
        Ok(CallToolResult::structured(with_session(
            json!({
                "artifact_id": artifact.id,
                "version": artifact.version,
            }),
            &resolved,
        )))
    }

    #[tool(description = "Publish a new version of an existing artifact.")]
    async fn artifact_update(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactUpdateParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Write)
            .await
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                Some(&project_id),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let session_id = resolved.session_id().map(str::to_string);
        let _gate = self.hold_gate(session_id.as_deref()).await;
        let artifact = artifacts::update_for_principal_capped(
            &self.state.db,
            &self.state.data_dir,
            Some(&principal),
            self.state.config.events_per_project.per_project,
            &principal.writer(session_id.as_deref()),
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
        // Tell a viewer of this artifact that its live version has sealed, so
        // the band clears and the document settles without a reload.
        self.state.notify_live(&artifact.id, artifact.version, 0);
        Ok(CallToolResult::structured(with_session(
            json!({ "version": artifact.version }),
            &resolved,
        )))
    }

    #[tool(
        description = "Write the version an artifact holds live, forking one when none is held. Use this while you are still working: each call rewrites the same version in place, so the version count grows by one per live session rather than one per write, and the artifact's page always serves the last version you sealed. Returns a viewer_url you can open in your browser to show the human as you work; it carries no credential. A version held by another session is a conflict unless force is set. Publishing with artifact_update seals the live version."
    )]
    async fn artifact_draft(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactDraftParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Write)
            .await
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                Some(&project_id),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let session_id = resolved.session_id().map(str::to_string);
        let _gate = self.hold_gate(session_id.as_deref()).await;
        let artifact = artifacts::draft_for_principal_capped(
            &self.state.db,
            &self.state.data_dir,
            Some(&principal),
            self.state.config.events_per_project.per_project,
            &principal.writer(session_id.as_deref()),
            &params.artifact_id,
            params.content.as_bytes(),
            match params.envelope {
                None => EnvelopeUpdate::Keep,
                Some(None) => EnvelopeUpdate::Clear,
                Some(Some(envelope)) => EnvelopeUpdate::Set(envelope),
            },
            LiveOptions {
                force: params.force,
                session_id: session_id.as_deref(),
            },
        )
        .await
        .map_err(to_error_data)?;

        if let Some(version) = artifact.live_version {
            self.state
                .notify_live(&artifact.id, version, artifact.live_rev);
        }
        let show = artifact.live_version.unwrap_or(artifact.version);
        let headers = request_headers(&context);
        let origin = crate::http::origin::request_origin(&self.state.config, &headers);
        Ok(CallToolResult::structured(with_session(
            json!({
                "artifact_id": artifact.id,
                "version": artifact.version,
                "live_version": artifact.live_version,
                "live_rev": artifact.live_rev,
                // The page is the artifact's own, so it needs no credential: an
                // artifact with no active share is already readable at this
                // address. The live version is named explicitly because the plain
                // address keeps serving the last sealed one.
                "viewer_url": format!("{origin}/artifacts/{}?version={show}", artifact.id),
            }),
            &resolved,
        )))
    }

    #[tool(description = "Read an artifact's metadata and its content, optionally at a version.")]
    async fn artifact_get(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactIdParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Read)
            .await
            .map_err(to_error_data)?;
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            Some(&project_id),
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let existing = artifacts::metadata(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
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
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Read)
            .await
            .map_err(to_error_data)?;
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            Some(&project_id),
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let versions = artifacts::list_versions(&self.state.db, &params.artifact_id)
            .await
            .map_err(to_error_data)?;

        Ok(CallToolResult::structured(json!({ "versions": versions })))
    }

    #[tool(
        description = "Squash an artifact's versions: remove every version before the named one, keeping it and everything after it."
    )]
    async fn artifact_squash(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactSquashParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Write)
            .await
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                Some(&project_id),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let _gate = self.hold_gate(resolved.session_id()).await;
        let target = params.version.unwrap_or(0);
        let target = if target < 1 {
            artifacts::metadata(&self.state.db, &params.artifact_id)
                .await
                .map_err(to_error_data)?
                .version
        } else {
            target
        };
        let removed = artifacts::squash_versions(
            &self.state.db,
            &self.state.data_dir,
            &params.artifact_id,
            target,
        )
        .await
        .map_err(to_error_data)?;
        self.state.notify();
        Ok(CallToolResult::structured(with_session(
            json!({
                "artifact_id": params.artifact_id,
                "version": target,
                "versions_squashed": removed,
            }),
            &resolved,
        )))
    }

    #[tool(description = "Delete an artifact and its history.")]
    async fn artifact_delete(
        &self,
        context: RequestContext<RoleServer>,
        Parameters(params): Parameters<ArtifactDeleteParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let principal = self.principal(&context);
        let project_id = artifacts::project_id(&self.state.db, &params.artifact_id)
            .await
            .map_err(|err| to_error_data(policy::conceal(&principal, err)))?;
        policy::authorize(&self.state.db, &principal, &project_id, Access::Write)
            .await
            .map_err(to_error_data)?;
        let resolved = self
            .resolve_session(
                &context,
                &principal,
                params.session.as_ref(),
                Some(&project_id),
                Mode::Write,
            )
            .await
            .map_err(to_error_data)?;
        let _gate = self.hold_gate(resolved.session_id()).await;
        let deleted = artifacts::delete(
            &self.state.db,
            &self.state.data_dir,
            &principal.writer(resolved.session_id()),
            &params.artifact_id,
        )
        .await
        .map_err(to_error_data)?;

        self.state.notify();
        Ok(CallToolResult::structured(with_session(
            json!({ "artifact_id": deleted.id }),
            &resolved,
        )))
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
        self.resolve_session(
            &context,
            &principal,
            params.session.as_ref(),
            Some(&params.project_id),
            Mode::Read,
        )
        .await
        .map_err(to_error_data)?;
        let author = author_filter(
            params.member.as_deref(),
            params.agent.as_deref(),
            params.kind.as_deref(),
        )
        .map_err(to_error_data)?;
        let listed = artifacts::list_with_session(
            &self.state.db,
            &params.project_id,
            params.session_id.as_deref(),
        )
        .await
        .map_err(to_error_data)?;
        let mut listed = serde_json::to_value(&listed)
            .map_err(|err| to_error_data(Error::Config(err.to_string())))?;
        if !author.is_empty()
            && let Some(rows) = listed.as_array_mut()
        {
            rows.retain(|row| super::author_matches(row, &author));
        }

        Ok(CallToolResult::structured(json!({ "artifacts": listed })))
    }
}

impl HubServer {
    /// Hold a session's gate shared for a write that touches the files it
    /// owns, so the session cannot end or be pruned under the write.
    async fn hold_gate(
        &self,
        session_id: Option<&str>,
    ) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        match session_id {
            Some(id) => Some(self.state.session_gates.shared(id).await),
            None => None,
        }
    }
}

/// Arguments for `artifact_publish`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactPublishParams {
    project_id: String,
    title: String,
    /// What the artifact holds: `html` for a page the browser renders,
    /// `markdown` for a document the viewer renders.
    ///
    /// The schema carries the two kinds rather than a bare string, so an agent
    /// reads them off the tool instead of guessing and being refused. The list
    /// is the one the store holds them against, so the two cannot drift.
    #[schemars(extend("enum" = crate::blob::KINDS))]
    kind: String,
    content: String,
    #[serde(default)]
    description: String,
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
    /// The session this write belongs to.
    #[serde(default)]
    session: Option<SessionArg>,
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
    #[serde(default)]
    session: Option<SessionArg>,
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

/// Arguments for `artifact_draft`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactDraftParams {
    artifact_id: String,
    content: String,
    /// The same three-way envelope as `artifact_update`: absent keeps the
    /// current envelope, an envelope replaces it, and an explicit null writes
    /// this version in the clear.
    #[serde(default, deserialize_with = "present_or_absent")]
    #[schemars(with = "Option<serde_json::Value>")]
    envelope: Option<Option<serde_json::Value>>,
    /// Take the live pointer from another session instead of conflicting.
    #[serde(default)]
    force: bool,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `artifact_get`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactIdParams {
    artifact_id: String,
    #[serde(default)]
    version: Option<i64>,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `artifact_versions`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactVersionsParams {
    artifact_id: String,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `artifact_delete`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactDeleteParams {
    artifact_id: String,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `artifact_squash`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactSquashParams {
    artifact_id: String,
    #[serde(default)]
    version: Option<i64>,
    #[serde(default)]
    session: Option<SessionArg>,
}

/// Arguments for `artifact_list`.
#[derive(Debug, Deserialize, JsonSchema)]
struct ArtifactListParams {
    project_id: String,
    /// The session this call belongs to. It does not filter the list.
    #[serde(default)]
    session: Option<SessionArg>,
    /// Only artifacts written in this session, by id.
    #[serde(default)]
    session_id: Option<String>,
    /// Only artifacts by this member.
    #[serde(default)]
    member: Option<String>,
    /// Only artifacts by this agent, such as `claude-code`.
    #[serde(default)]
    agent: Option<String>,
    /// Only artifacts by this kind of author: `participant`, `human`, `local`
    /// or `legacy`.
    #[serde(default)]
    kind: Option<String>,
}
