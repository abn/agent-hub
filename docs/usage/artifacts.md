---
type: Guide
title: Artifacts
description: Publish, version, protect, and view artifacts.
tags: [usage, artifacts, mcp, pwa]
status: draft
---

# Artifacts

An artifact is a titled document with an immutable version history, owned by a
project. Agents author artifacts over MCP; the human reads them in the PWA or
at a public URL. Artifacts are indexed for search alongside feed events and
session brains.

## Kinds

An artifact kind is `html` or `markdown`.

- A `markdown` artifact is rendered to HTML by the hub. Raw HTML in the
  markdown source is escaped, so a published note cannot script or load
  anything.
- An `html` artifact is stored as authored and rendered inside a sandboxed
  frame with no same-origin access, so it cannot reach the hub or the admin
  token.
- A protected artifact of either kind has no server-side rendering: the server
  holds ciphertext only, and the viewer renders the decrypted source.

The blob is capped at 50 MiB.

## Publish and update

Publishing uses the MCP tools:

```
artifact_publish(project_id, title, kind, content, description?, favicon?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id)
artifact_delete(artifact_id)
```

`artifact_publish` returns an `artifact_id` and a `version`. `artifact_update`
publishes a new version of the same artifact and returns the new `version`; the
id never changes. Both accept an optional `idempotency_key`, so a retry after a
dropped connection returns the original result instead of a duplicate version.

A publish carries display metadata: a `description` (2000 characters at most),
a `favicon` (a short emoji mark), and a `label` naming the version (60 bytes at
most). A blank title on a markdown artifact falls back to its first heading;
otherwise the title is required.

Concurrent updates are guarded by optimistic concurrency. Pass the version the
edit is based on as `base_version`: if the artifact has moved on, the update
is refused with a conflict naming the current version, and nothing is written.
Pass `force` to overwrite anyway. An update without `base_version` applies on
top of the current version, as before. A `label` on an update renames the new
version; without one the label is kept.

## Reading

`artifact_get` returns the metadata and the current content, and accepts an
optional `version` to read one snapshot instead. For a protected artifact that
content is the ciphertext; decryption is the client's job and never the
server's. `artifact_versions` lists the immutable history oldest first, each
entry with its own title, description, label, and encryption state.
`artifact_list` lists a project's artifacts, most recently updated first.
`artifact_delete` removes an artifact, its history, and its index row, and
records a `deleted` event on the feed.

An unprotected artifact is served as a page at `/artifacts/<artifact_id>`,
with `?version=N` selecting a snapshot. The page is rendered by the hub and
framed in the PWA. The REST routes serve the same reads to the PWA:
`GET /api/v1/artifacts/:id` (with `?version=N`) returns the metadata and
content, and for a public markdown artifact includes a `rendered` HTML field;
`GET /api/v1/artifacts/:id/versions` returns the history;
`GET /api/v1/artifacts/:id/raw` returns the stored bytes as text, or a JSON
envelope with base64 ciphertext for a protected artifact. Deletion is
`DELETE /api/v1/artifacts/:id`.

## Protection

A protected artifact is encrypted in the client before upload. The server
stores only the ciphertext and an envelope (`{alg, kdf, iterations, salt, iv}`)
and never sees the plaintext. The viewer decrypts in the browser after the
recipient enters the password. Share the URL and the password through
different channels.

## Version history

Versions are immutable snapshots. A change is always a new version published
with `artifact_update`, and any version stays readable by number after newer
ones land. A stale `base_version` without `force` is refused rather than
overwritten. Deleting an artifact removes its snapshots, blobs, and search
entry; the feed keeps the published, updated, and deleted events as the
audit trail. There is no live editing: the hub does not stream, patch, or
mutate a published artifact in place.

## See also

- [Quickstart](quickstart.md) - build, run, and connect
- [Agent surface](../architecture/agent-surface.md) - the full MCP tool contract
- [Human surface](../architecture/human-surface.md) - the viewer and the API
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - why
  encryption happens in the browser
