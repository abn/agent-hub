# Resource URLs

When your output names a hub resource, give its URL, not the bare id. A chat
reply, a PR, a commit, an inbox item or an artifact's text then links the
human straight to the thing. Use the `url` a tool returned. When you only hold
an id, build the URL.

`<hub>` is the base URL `whoami` reports, the same one the bootstrap names.

| Resource | URL |
|---|---|
| Artifact, feed event (question, approval, signal, answer), session, comment | `<hub>/id/<id>` |
| Knowledge base page | `<hub>/#/projects/<project>/wiki?page=<path>` |
| Project feed | `<hub>/#/projects/<project>/feed` |

`<hub>/id/<id>` needs no kind. The hub resolves the id and opens the right
view in the app. A person without access, or an id that does not exist, sees
the same not-found state. A page path is the stored one, such as
`/fs/notes/plan.md`, percent-encoded: `?page=%2Ffs%2Fnotes%2Fplan.md`.

Tools that create or return a resource carry a `url` string beside its id:
`artifact_publish`, `artifact_update`, `artifact_draft`, `artifact_get`,
`question_post`, `answer_post` (the question's URL), `signal_append`,
`comment_post`, `session_start`, `session_resume` and `brain_promote` (the knowledge base
page). Pass that `url` on as it is.
