---
type: Decision Record
title: An installable PWA is the human surface
description: The human interface is a responsive, mobile-first PWA served from the same binary.
tags: [adr, pwa, ui, human]
status: stable
---

# 0005. An installable PWA is the human surface

## Context

The human supervises the fleet mostly from a phone and sometimes from a
desktop. A native application per platform multiplies build and release
surface, which fights the lean single-binary goal.

## Decision

The human surface is an installable PWA: responsive and mobile-first,
installable to a phone home screen, and equally good in a desktop browser.
The binary serves it as static assets.

## Consequences

- One UI artifact for every device, shipped with the binary.
- No app store, no per-platform build.
- The PWA is the window into the hub: inbox, project feeds, artifact gallery
  and viewer, session explorer, search, and storage and prune.
- Notifications are an opt-in local notification raised while the app runs;
  background push and in-app badges remain later layers.
