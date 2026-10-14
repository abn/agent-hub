---
type: Guide
title: Documentation
description: How the wiki is written, checked, and kept public-ready
tags: [contributor, contribution]
status: stable
---

# Documentation

The wiki under `docs/` is an Open Knowledge Format (OKF) v0.2 bundle: a tree of
Markdown pages with YAML frontmatter, plus two reserved files a directory uses
for its listing and its history. This page says how to add or change a page, how
the bundle is checked, and how the screenshots stay current. By the end you will
be able to write a conforming page and land it with the gate green.

## Before you start

| Thing | Why |
|---|---|
| The `okf` validator | `make docs/check` runs it over the bundle |
| The page's `description` | the parent index reuses it verbatim |

Install the validator once:

```sh
GOBIN="$HOME/.local/bin" GOTOOLCHAIN=auto go install github.com/okfcli/okf/cmd/okf@latest
```

## Write a page

Every page but an index opens with a frontmatter block. `type` is the only
required key; the rest are the conventions this bundle keeps.

```yaml
---
type: Guide
title: Deploy as a service
description: Run the hub under systemd with Podman Quadlet or Docker
tags: [operator, operations, install]
status: stable
---
```

- **`type`** is one of `Overview`, `Tutorial`, `Guide`, `Reference`, `Runbook`,
  `Explanation`, `Decision Record`, or `Gallery`. It picks the shape: a Guide is
  one task in prose, a Reference is a table you scan.
- **`title`** is the page's heading.
- **`description`** is one clause, under 90 characters, with no trailing period.
  The parent index reuses it verbatim, so it is written once.
- **`tags`** are the audience and the topic; the vocabulary is below.
- **`status`** is `stable` for behaviour that ships and `draft` for intended
  design that does not. Nothing else.

### The tag vocabulary

Exactly one audience tag, then one to three topic tags.

- Audience: `user`, `operator`, `agent-author`, `contributor`.
- Topic: `overview`, `install`, `config`, `mcp`, `rest`, `pwa`, `sessions`,
  `feed`, `inbox`, `knowledge-base`, `artifacts`, `search`, `operations`,
  `security`, `design`, `testing`, `contribution`, `notifications`.

Keep the set small: two tags that mean the same thing produce two tag pages and
halve the value of both.

## Indexes and the log

An `index.md` carries no frontmatter. The bundle root is the one exception: it
carries `okf_version: "0.2"` and nothing else. An index is a `# Title` heading
and a curated bulleted list, and each entry reuses the page's own description:

```
* [Deploy as a service](/operate/deploy-as-a-service.md) - Run the hub under systemd with Podman Quadlet or Docker
```

The `log.md` at the bundle root records changes to the bundle itself, newest
date first under `## YYYY-MM-DD` headings, each bullet starting `* **Update**:`,
`* **Creation**:`, or `* **Deprecation**:`. It is not a software changelog and
not a progress journal.

A new page is three coupled edits in one commit: the page, its entry in the
parent index, and a line in the log.

## Links and images

Cross-links are bundle-relative, of the form `/section/page.md` with a leading
slash from the bundle root, so a link does not depend on where its own page
sits.

Images are raw HTML `<picture>` elements, because the renderer does not rewrite
raw HTML and the site is served under a base path. The `src` is relative to the
page: a bundle-root page writes `assets/screens/x.png`, a page one directory
deep writes `../assets/screens/x.png`, and one two directories deep writes
`../../assets/screens/x.png`. Each pair follows the reader's colour scheme:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/screens/home-desktop-dark.png">
  <img src="../assets/screens/home-desktop-light.png" alt="Home at desktop width.">
</picture>
```

> [!WARNING]
> Never write an image with Markdown syntax: the validator reads it as a broken
> concept link. Never use a leading-slash image path either, which is not
> rewritten and breaks under the base. Use the phone pair where the page is
> about a phone, and the desktop pair otherwise.

## Screenshots

`docs/assets/screens/` holds the main feature screens at both widths (1440x900
desktop, 390x844 phone) in both themes, captured from a scratch hub seeded with
dummy data. [Product tour](/screens.md) embeds them all. The capture skill under
`.agents/skills/capture-wiki-screenshots/` drives a seeded hub and writes the
files, and each capture is looked at before it lands. Re-run and commit the
captures when a screen changes, in the same commit as the change.

## The gate

`make docs/check` validates the bundle with `okf validate docs`; install the
validator as above. `pre-commit` runs the same validator whenever a file under
`docs/` changes, so a page whose frontmatter does not parse, or that carries no
`type`, fails before it lands.

> [!IMPORTANT]
> Docs move with the change. A behaviour change updates the wiki and its log in
> the same commit, and the internal progress log is updated separately and never
> in the bundle.

## House style

- No em dash and no en dash. Use a comma, a colon, or two sentences.
- No emoji. Icons are inline SVG line glyphs or typographic marks.
- Public-ready by default: no internal names, codenames, hostnames, absolute
  paths, tokens, or task identifiers.
- Structure with `##` sections. They become the on-page table of contents, and
  the search index is built from heading-anchored chunks, so five short sections
  beat one wall of text.
- Callouts (`> [!NOTE]`, `> [!TIP]`, `> [!IMPORTANT]`, `> [!WARNING]`,
  `> [!CAUTION]`) render as cards. Use them for prerequisites, warnings, and
  shipped-versus-intended.
- Define jargon on first use, or link [Glossary](/reference/glossary.md).

## See also

- [Documentation update log](/log.md) - how the bundle has evolved
- [Product tour](/screens.md) - every captured screen
- [Contributor guide](/contribute/guide.md) - the workflow and the gates
