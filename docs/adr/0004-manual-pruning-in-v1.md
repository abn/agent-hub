---
type: Decision Record
title: Manual pruning in v1
description: No automatic expiry ships; the human is the garbage collector, and retention is a per-layer concept.
tags: [adr, retention, lifecycle]
status: stable
---

# 0004. Manual pruning in v1

## Context

Session brains and feed events grow without bound. A retention policy could
ship from day one, but an automatic policy that deletes agent state is a
sharp tool, and its right settings are unknown before real use.

## Decision

Ship no automatic expiry in v1. The human prunes sessions and projects
explicitly through the PWA, which is the only deletion path. Retention is
treated as a per-layer concept, with separate later policies for feed events,
session brains, and artifacts.

## Consequences

- The schema carries `created_at`, `last_activity`, a `retention` column, and
  room for an `archived_at` column, so the later layers slot in without a
  painful migration.
- The storage and prune screen is a first-class part of the PWA, not an
  afterthought.
- Session brains survive until the human acts, which is safe but requires the
  prune surface to be usable early.
