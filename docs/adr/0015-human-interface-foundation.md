---
type: Decision Record
title: The human interface follows the design foundation
description: A calm, mailroom-like PWA with a fixed token set, a no-emoji rule, and an accessibility build gate.
tags: [adr, pwa, design, accessibility]
status: stable
---

# 0015. The human interface follows the design foundation

## Context

The human surface is where the operator reads, answers, approves, and prunes.
It needs to feel calm by default and step louder only when something waits,
without becoming an alarm panel. A complete design foundation was produced
with final tokens, components, screens, states, and copy.

## Decision

The interface follows the design foundation. Warm paper in light, warm
charcoal in dark, one humanist sans, monospace only for literal data, and a
fixed token set reused verbatim. Eight screens (Home, Inbox, Project feed,
Artifacts, Sessions, Search, Storage, Project settings), with agent and access
management as a sub-screen of settings, a four-tab mobile bar, and a list plus
detail layout on desktop. The interface is static assets served by the binary,
with no CDN and no runtime dependency.

## Consequences

- Prune is reversible for 30 seconds, and read is explicit, never scroll-past.
- The alert hierarchy has three levels, L0 to L2, and alerts are never red,
  never animated, and never a modal.
- No emoji anywhere; icons are inline SVG line glyphs or typographic marks.
- Accessibility is a build gate: WCAG AA in both themes, a 12px UI text floor,
  44px tap targets, visible focus, a full keyboard path, and no colour-only
  meaning. Reduced motion is honoured.
- The design is framework-agnostic; the smallest static stack that keeps the
  bundle small is chosen when the interface is built.
