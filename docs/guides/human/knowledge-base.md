---
type: Guide
title: Knowledge base
description: Review, edit, and promote the durable pages a project keeps
tags: [user, knowledge-base]
status: stable
---

# Knowledge base

Every project has one knowledge base: a tree of markdown pages under `/fs/`
that outlives every session, shared by every agent with write access. You read
and review it as a wiki under the project's **Wiki** tab. This page is the
human's half; the page format, paths, limits, and the routes are in
[Knowledge base pages](/reference/knowledge-base-pages.md), and the agent's
tools are in [MCP tools](/reference/mcp-tools.md).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../../assets/screens/wiki-desktop-dark.png">
  <img src="../../assets/screens/wiki-desktop-light.png" alt="A project wiki with its page tree.">
</picture>

## The wiki

The tree is the whole knowledge base in one listing, drawn as a full tree on a
desktop and one drill-in level on a phone. A page row carries its title and a
mono line of its type, status, and derived trust; a stale page adds the word
"stale", and a directory row carries its child count. Selecting a page renders
it: the frontmatter block is metadata, shown as the page's type, status, and
tags rather than as body text, and the backlinks are listed under it.

## A page

A page is markdown with a YAML frontmatter block. The hub reads it, and the wiki
shows `type`, `status`, and `tags` as metadata; `title` labels a listing, and
`stale_after` is the date after which the page reads as stale. `type` is the one
field a page needs. Any other top-level key is kept and ignored, so a page can
carry fields of its own. A page with no block is stored as sent and flagged as
needing one, so nothing is lost to a missing field. The full key list is in
[Knowledge base pages](/reference/knowledge-base-pages.md).

## Review and trust

A page's trust is derived, never stored: `unverified` when nobody has reviewed
it, `human_reviewed` when the newest review was yours, `machine_confirmed` when
it was another agent's, and `edited_since_review` when the page no longer holds
the bytes that were verified.

**Review** stamps the page you have read, carrying the version you read. If the
page changed under you, the review is refused and nothing is stamped. **Needs
review** lists the pages the hub judges not human-reviewed, and a row opens the
page.

## Editing a page

**Edit** opens the editor, whose Save carries the version token the read
carried. A page that changed under the editor is refused rather than
overwritten, and the refusal is shown in place with **Reload theirs** and
**Keep mine**. **New page** writes a page that is not there yet.

## History, revert, and restore

A page's **History**, opened from the reader's control row, lists its versions
newest first with who, when, and what each did, and marks the version the page
holds now. A version opens as a line diff against the page as it is now.
**Revert to this** writes that version back after a confirmation; a revert
deletes nothing, so it is not a danger action and can itself be reverted. A diff
too long to work out line by line says so and shows the version itself.

A deleted page's address says it was deleted and links to its history, where
**Restore this** writes it back the same way. **Forget history**, in the
history's header, drops the kept bytes after a dialog that names the page; the
rows stay, so the history still says who wrote what and when, but the versions
can no longer be read or restored. Forgetting a history is an admin action, like
prune.

## Promoting session knowledge

A session's working state is pruned with the session, so durable knowledge is
promoted out. A session brain entry in the session detail offers **Save to
wiki**, which copies it into the knowledge base as a page that cites the session
it came from and leaves the source where it was. An agent does the same with the
`brain_promote` tool.

## Housekeeping

The Wiki home carries the page, needs-review, and stale counts, and holds:

- **Recent changes**, which reads who did what to which page and when, grouped
  by day, with rows that open that page's history;
- **Lint**, the whole tree's findings, with a Re-check that walks it again.

## What the wiki does not do

There is no rename or move: a move is a read, a write at the new path, and a
delete, and it starts a new history because history is keyed by path. There is
no rendered-compare merge; a conflict offers the hub's copy or yours. Pages
carry no comment threads. Wiki links (`[[page]]`) are not links to the hub; only
markdown links are followed. Offline reading is not shipped: what the service
worker caches is the app shell and its assets, not page content, so reading
pages needs a connection.

## You should now see

The project's wiki, with a page tree, a reader that shows metadata and
backlinks, and the housekeeping screens for review, history, and lint.

## See also

- [Knowledge base pages](/reference/knowledge-base-pages.md) - the page format, paths, limits, and routes
- [Sessions and brains](/guides/human/sessions-and-brains.md) - the session detail that promotes a page
- [Bring existing notes over](/guides/agents/migrate-notes.md) - move a harness's notes into pages
- [Search](/guides/human/search.md) - find a page across the tree
