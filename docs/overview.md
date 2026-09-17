---
type: Reference
title: Overview
description: What Agent Hub is and why it exists.
tags: [project, introduction]
status: draft
---

# Overview

This page describes what Agent Hub is for and how it is shaped. See
[goals and non-goals](design/goals.md) for the v1 boundary and the
[architecture](architecture/index.md) for the components.

## The problem

Agents lose working state when their context is compacted or the client
restarts. On top of that, a fleet that runs across several machines has no
shared, time-ordered account of what happened, and the human who supervises it
has no single calm queue for finished work and approval requests. Vendor
clouds solve some of this, but they move the data and the brain off the
machine.

## What Agent Hub is

Agent Hub is the self-hosted answer. It is one lean binary or container that
runs on a homelab or NAS node and acts as the central, cross-node hub for a
fleet of agents and their human. The node is the cloud: agents on any machine
report in over a LAN or tailnet, and there is no remote brain and no vendor
service.

It offers four surfaces over one data model:

- **Session brains.** Each agent session gets a server-side, session-scoped
  store. Its life is bound to the session, so it survives same-session
  compaction and resume of the same named session.
- **A project feed.** A time-ordered, append-only, addressable event store per
  project. Agents read it on startup; the human reads it as a feed.
- **A global inbox.** One queue for the human: finished work, questions, and
  approvals, under async mailbox semantics.
- **Artifacts.** Hosted HTML and markdown outputs, with browser-side
  encryption and password sharing for protected content.

Agents reach the hub over the Model Context Protocol (MCP). The human reaches
it through an installable, mobile-first PWA.

## How it is built

Two choices shape everything else. First, a single storage engine: the Turso
Database Rust engine backs the hub event store, the per-session AgentFS files,
the search index, and artifact metadata, so there is no two-engine split;
artifact blobs live on the data volume. Second, real AgentFS: the hub wraps
it rather than reimplementing it, giving each session one file that holds
key-value state, an append-only audit log, and a POSIX-like filesystem.

The result is lean by construction: one process, one engine, no
microservices, no CRDTs, and no custom distributed consensus. See the
[decision records](adr/index.md) for the reasoning.

## Status

One binary opens the engine and serves the REST API, the installable PWA, and
MCP on one listener. The feed, session brains, the inbox and questions,
artifacts with reversible prune, engine-native search, per-agent identity with
a trust model, and the container packaging exist. The session detail view,
push notifications, project deletion, and an embedded tailnet are intended
design, and the pages describing them say so.
