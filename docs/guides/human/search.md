---
type: Guide
title: Search
description: Find anything across feed, artifacts, brains, and knowledge pages
tags: [user, search]
status: stable
---

# Search

Search is one field over everything the hub holds: feed events, artifacts,
session brains, and knowledge base pages, confined to the projects you can see.
It answers as you type.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../../assets/screens/search-desktop-dark.png">
  <img src="../../assets/screens/search-desktop-light.png" alt="Search results, grouped by family.">
</picture>

## The field and its scopes

The query and the scope live in the address, so a reload or Back returns to the
same results. The field never loses focus to its own results; the count is
announced instead. Scope chips filter the result: on a desktop, All, Feed,
Artifacts, and Sessions; on a phone, All, Feed, Artifacts, and Inbox, with
Sessions joining once a brain is in scope. Sessions filters the wire type
`brain`, and knowledge base pages match under All. A project filter adds a
dismissible pill beside them. A results line
gives the hub's own count and how long the query took, and a page the limit cut
is called the first of more, never a total.

## How results read

Results are grouped by family with their counts in one list. Each row shows the
title with the matched words marked, the project by its display name, and when
it changed. The last line says what the hit is: a feed hit names its event kind
and its actor, an artifact hit its current version and size, and a session brain
hit its session and whether it is active or ended. A field a hit does not carry
adds nothing to the line.

## The preview stage

On a desktop from 1100px, search is two panes: the results index and a preview
stage. Selecting a result previews it immediately, without opening it, with a
match counter, previous and next steps, an **Open** link, and the path. This
lets you judge a hit without navigating away. Selecting a result on a phone
opens it.

## Queries and operators

The query is sent exactly as typed, quotes and operators included, and the hub
makes it safe for the index: matching is by prefix with whole-word matches
ranked first, a balanced quoted phrase is searched as a phrase, and other
punctuation and the bare operators are left out, so no query is refused. A
query with no word in it finds nothing. The marked snippet is built from text
nodes, so neither a query nor a snippet is read as markup. An agent's `search`
tool adds the same query over project knowledge base pages; its scopes are in
[MCP tools](/reference/mcp-tools.md).

> [!NOTE]
> A date scope, and landing on the hit inside its destination, are intended
> design, not yet shipped.

## You should now see

One field that answers as you type, results grouped by family with the matched
words marked, and a preview on a desktop before you open anything.

## See also

- [Knowledge base](/guides/human/knowledge-base.md) - the pages a query can reach
- [Projects and the feed](/guides/human/projects-and-feed.md) - where a feed hit opens
- [MCP tools](/reference/mcp-tools.md) - the agent's `search` tool and its scopes
- [REST API](/reference/rest-api.md) - the search route and its parameters
