---
type: Decision Record
title: Human idea capture surface
description: Where and how the human captures an idea into a project's inbox, aligned with the design frame.
tags: [adr, ideas, inbox, ui, human-surface]
status: draft
---

# 0034. Human idea capture surface

## Status
Draft (companion to the idea capture endpoint in `POST /api/v1/projects/{id}/ideas`).

## Context
The idea capture backend lands an idea as a needs-action signal in a project's
inbox. A human surface is still needed: a place to write the idea, choose its
project, and see it triaged, without leaving the mailbox model.

## Decision
- The capture entry point is a single glyph button in the inbox/project header
  control row, opening a sheet (phone) or the aside (desktop) per the one-
  surface rule.
- The sheet holds one 640px column with a summary field and an optional note,
  sized by the pointer like every other form.
- On submit the idea is posted to the project's endpoint and appears in the
  inbox without a reload; the composer clears and focus returns to the button.
- An empty summary is refused client-side and server-side.
- Triage reuses the existing inbox verbs: read, mark done, and (later) promote
  to a knowledge base page.

## Consequences
- No new screen: capture is an aside/sheet over the inbox or project stage.
- Keeps the frame stable and avoids a dedicated route.
- The button is the only new control; all sizing follows the design tokens.
