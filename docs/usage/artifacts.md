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
artifact_publish(project_id, title, kind, content, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, idempotency_key?)
artifact_get(artifact_id)
artifact_list(project_id)
```

`artifact_publish` returns an `artifact_id` and a `version`. `artifact_update`
publishes a new version of the same artifact and returns the new `version`; the
id never changes. Both accept an optional `idempotency_key`, so a retry after a
dropped connection returns the original result instead of a duplicate version.

## Reading

`artifact_get` returns the metadata and the current content. For a protected
artifact that content is the ciphertext; decryption is the client's job and
never the server's. `artifact_list` lists a project's artifacts, most recently
updated first.

An unprotected artifact is served as a page at `/artifacts/<artifact_id>`,
rendered by the hub and framed in the PWA. The REST route
`GET /api/v1/artifacts/:id` returns the metadata and content to the PWA, and
for a public markdown artifact includes a `rendered` HTML field.

## Protection

A protected artifact is encrypted in the client before upload. The server
stores only the ciphertext and an envelope (`{alg, kdf, iterations, salt, iv}`)
and never sees the plaintext. The viewer decrypts in the browser after the
recipient enters the password. Share the URL and the password through
different channels.

## Version history

Versions are immutable snapshots. A change is always a new version published
with `artifact_update`. There is no live editing: the hub does not stream,
patch, or mutate a published artifact in place.

## See also

- [Quickstart](quickstart.md) - build, run, and connect
- [Agent surface](../architecture/agent-surface.md) - the full MCP tool contract
- [Human surface](../architecture/human-surface.md) - the viewer and the API
- [Decision 0009](../adr/0009-browser-side-artifact-encryption.md) - why
  encryption happens in the browser
