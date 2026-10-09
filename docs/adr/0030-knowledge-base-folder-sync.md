---
type: Decision
title: A knowledge base syncs with a folder under the version guard
description: Why a project knowledge base exports to a plain folder with a manifest of page versions, and why an import lints first, skips a page the hub changed since, and deletes nothing unless asked.
tags: [adr, knowledge-base, okf, cli]
status: stable
---

# 0030. A knowledge base syncs with a folder under the version guard

## Context

A project knowledge base is an OKF bundle held in one AgentFS file
([ADR 0018](0018-project-knowledge-base.md)). An operator who wants to edit it
in their own editor, diff it, review it in a pull request or keep it in git has
had to read and write it one page at a time. A folder is the natural shape for
that, but a folder is a copy: while it is out, agents and the human keep writing
the pages on the hub, and a naive import would put the folder's older bytes over
their work without a word.

Every page write already takes an optional `if_version` guard, and a version is
the SHA-256 of the page's bytes, so a client can tell from the bytes alone
whether a page changed.

## Decision

`agent-hub kb export --dir D` writes every page at its path under `/fs` into D,
beside a manifest, `.agent-hub-kb.json`, that records the project and the
version each page had. `agent-hub kb import --dir D` reads the folder back. Both
are client commands over the same brain tools as the rest of `agent-hub kb`, on
one connection; the hub gains no route.

An import lints the whole folder with the hub's own OKF lint once it has
planned its changes, and refuses with nothing written when a finding says a
page it would create or update is wrong: `unparsed_frontmatter`,
`okf_version_misplaced` or `link_escapes_bundle`. The same finding on a page
the import leaves as it is is reported and does not refuse, because the hub
already holds that page and a knowledge base's own export has to import back
whatever its pages hold. The findings that say a bundle is incomplete,
`broken_link` among them, are reported and never refuse, because the hub's
lenient writes leave them as a matter of course.

Each page is then compared three ways, by version: in the folder, on the hub,
and in the manifest. A page the folder changed and the hub did not is written
with the manifest's version as its guard, or with `absent` when the export did
not have it. A page the folder still holds as exported, which the hub changed
or deleted since, is the hub's newer copy: it is left as it is, reported, and
is not a conflict, so an import of an old folder never fails on pages it does
not touch. A page both sides changed is a conflict: it is skipped, the other
pages are still written, and the command exits 1. So is a page `--prune`
would delete that the hub changed since the export. A write the
guard refuses because the page changed during the import is the same conflict.
Any other refusal from the hub stops the import there, and what was made and
what was not attempted are both reported. After an import the manifest records
what was written, and forgets a page gone from both the folder and the hub, so
the next import starts from there. A manifest whose keys are not canonical page
paths, or name one page twice, is refused, since it would guard one page with
another's version.

A page missing from the folder is kept. With `--prune` it is deleted, under the
same guard, and only when the manifest names it, so a page created on the hub
after the export is never pruned. `--dry-run` plans the same changes and writes
nothing. An export refuses a folder that is not empty unless `--force` is
given, and then removes only the files of pages its old manifest names and the
hub no longer has, so an import does not bring them back; a folder exported
from another project is refused even then, so its files are never removed.

Both directions refuse two page paths that differ only in case or Unicode
normalisation, and an import also checks the folder's pages against the hub
pages it keeps, reading each file name as its NFC spelling. A filesystem that ignores them, as macOS does by default, keeps
the two as one file, and an import would then write one page's bytes to the
other or prune the wrong one.

## Consequences

- A page is never silently overwritten or deleted by an import; the worst case
  is a conflict to resolve by hand, by exporting again or merging the files.
- There is no merge. A conflict names the page and leaves both copies as they
  were.
- The case check is Unicode lowercasing after NFC normalisation. It catches
  what macOS folds in practice, but it is not the filesystem's own table, so a
  rare pair it does not fold is still the operator's to avoid.
- The folder can be committed to git: dot-named files and directories are not
  pages, so `.git` and the manifest stay out of the bundle, and a page whose
  path has a dot-named component is left out of an export with a note.
- An import holds the folder in memory to lint it whole, so a bundle carries at
  most 10,000 pages and 256 MiB, and each page keeps the 1 MiB page limit.
- The data moves only where the operator points it: an export writes to a local
  directory and nowhere else.
