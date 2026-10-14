---
type: Guide
title: Bring existing notes over
description: Move a harness's existing notes into the project knowledge base
tags: [agent-author, knowledge-base]
status: stable
---

# Bring existing notes over

An agent that has been keeping notes under `~/.claude`, `~/.opencode` or
`~/.gemini` does not lose them: they move into the hub as pages. By the end you
will have the durable notes written into a project, one page per topic.

> [!IMPORTANT]
> There is no bulk importer, and this is manual. One write per topic is the
> whole of it, and there is no automatic sync back to the tool's home
> directory.

## Choose the store

Two stores take the content, and the choice is the working state versus durable
knowledge split:

- Durable knowledge, the thing the next agent or the next session needs, goes to
  the project knowledge base as a page under `/fs/`. This is where most notes
  that outlive a session belong.
- Working state, the scratch that belongs to one session and should not outlive
  it, goes to that session's brain with `store: "session"`.

Write a durable page once, from a tool or from a shell:

```
brain_put(path: "/fs/runbooks/deploy.md", store: "project",
          content: "<the note>", if_version: "absent")
```

```sh
agent-hub kb put runbooks/deploy.md --file deploy.md
```

`if_version: "absent"` creates the page only when nothing is there, so a second
agent running the same recipe does not clobber the first. The page is indexed
for search as soon as it is written.

## One page per topic

A single page per topic is the shape to aim for, not one page per file: the
knowledge base is a corpus that search reads and a wiki a human reviews, so
`/fs/runbooks/deploy.md` is more useful than `/fs/notes-2026-01-04.md`.

## Give the page a frontmatter block

A page is markdown with a YAML frontmatter block the hub reads, and the wiki
shows its `type`, `status`, and `tags` as metadata rather than body text. A page
with no block is stored as it was sent and comes back flagged with the minimum
to add, so nothing is lost to a missing field. `agent-hub kb put --help` prints
the fields the client knows, and the full key list is in
[Knowledge base pages](/reference/knowledge-base-pages.md).

## You should now see

The moved notes as pages under the project's Wiki tab, each readable, editable,
and findable by search, with the original files left where they were.

## See also

- [Knowledge base](/guides/human/knowledge-base.md) - the wiki a human reads and reviews
- [Knowledge base pages](/reference/knowledge-base-pages.md) - the page format and paths
- [One-shot calls from a hook](/guides/agents/hooks.md) - `agent-hub kb` from a shell
- [Using the hub as a brain](/guides/agents/operating.md) - the two stores and when to use each
