# Artifacts

An artifact is a titled blob with an immutable version history. Publish one
with `artifact_publish`, then publish a new version with `artifact_update`; the
artifact id stays the same and the version increments. Kinds are `html` and
`markdown`, and the content is capped at 50 MiB. Over HTTP the whole tool call
has to fit the transport limit as well, so content that needs a lot of JSON
escaping, such as minified markup full of quotes, has less than 50 MiB of room.

```
artifact_publish(project_id, title, kind, content, description?, label?, envelope?, idempotency_key?)
artifact_update(artifact_id, content, envelope?, base_version?, force?, label?, idempotency_key?)
artifact_draft(artifact_id, content, envelope?)
artifact_get(artifact_id, version?)
artifact_versions(artifact_id)
artifact_list(project_id, session?)
artifact_delete(artifact_id)
comment_post(artifact_id, body, anchor?, anchor_version?, idempotency_key?)
comment_list(artifact_id)
comment_resolve(artifact_id, comment_id, done, delete_token?)
comment_delete(artifact_id, comment_id, delete_token?)
```

A publish carries a description and a version label. An
update keeps the existing label when omitted; an explicit null or empty string
clears it. Pass the version the edit is based on as `base_version`: a stale base
is refused with a conflict naming the current version unless `force` is set.
Read one snapshot with `artifact_get` plus `version`, list history with
`artifact_versions`, and remove an artifact with `artifact_delete`.
Comment with `comment_post` (a point or quote anchor is optional), read
with `comment_list`, and resolve or delete with the returned delete token
or write access. Quotes are refused on protected versions.

A public artifact is served as a page at `{{base_url}}/artifacts/<artifact_id>`
with `?version=N` selecting a snapshot, and rendered in the PWA. Markdown
artifacts are rendered in the browser by the viewer, with raw HTML in the
source escaped; the viewer frames every artifact without same-origin access.

Sharing is the operator's act, not yours: an agent publishes, versions and
protects an artifact, and the human issues or revokes a share link from the
PWA. There is no MCP tool to create or revoke a link.

For protected content, encrypt in the client and send the ciphertext as
`content` with its `envelope`:

```
{ "alg": "AES-256-GCM", "kdf": "PBKDF2-HMAC-SHA256",
  "iterations": 600000, "salt": "...", "iv": "..." }
```

The server stores the envelope and ciphertext and never sees the plaintext. A
protected artifact has no server-side rendering; the viewer decrypts it in the
browser. It accepts `iterations` from 100000 to 10000000 and refuses anything
outside that range before the password is tried, so such an artifact never
opens.

An update treats `envelope` three ways: leave it out and the artifact's current
envelope carries forward, pass one and this version is protected under it, or
pass `null` and this version is published in the clear, with `content` as
plaintext. Older versions keep what they were published as, so an artifact can
hold a protected version and a plain one.

A project can decide this for you. Most projects leave it to you, but one set
to require protection refuses a publish or an update with no `envelope`, and
one that keeps its artifacts in plain text refuses a publish or an update that
would carry one. Either refusal is `invalid_argument` and names what to send
instead; nothing is written. If the artifact is already protected and the
project has since turned protection off, the refusal says to send
`envelope: null` with the plaintext content, which moves it into the clear
without losing the id, the history or the comments.

Authored HTML has a strict content security policy, so it cannot make external
requests: inline all CSS and JS, embed images and fonts as `data:` URIs, keep
no storage-backed state, support light and dark themes, avoid horizontal body
scroll, and use no emoji or em-dashes.

## Live editing

A version can be held live while you write it. Start one with
`artifact_draft`: it mints the next version, points the artifact at it, and
returns a `viewer_url`. Every later `artifact_draft` overwrites that one version
in place, so one version is minted per session rather than one per save, and the
page shows your latest bytes on a reload with no extra call.

Open the `viewer_url` in your own browser tool and keep it open as you work. The
human can watch the document take shape while you write it, and can comment or
steer in the session meanwhile; comments and answers reach you on your next
`artifact_draft` through the notification trailer. The URL carries no
credential, because a plain artifact with no active share is already public. If
the human has shared this artifact, its page is concealed and your URL returns
not found: that is deliberate, and sharing is the human's act, so publish and
let them open it.

Publishing with `artifact_update` seals the version: it becomes the current
version, immutable like any other, and is indexed for search. Until then the
artifact's default public URL keeps serving the last sealed version, so a link
the human has already shared never shows work in progress.

If another agent already holds a version live, `artifact_draft` is refused with
a conflict naming it; pass `force` to take it over.
