---
okf_version: "0.2"
---

# Agent Hub documentation

Agent Hub is a local-first operations layer for AI agents and their humans,
shipped as one lean binary or container on a homelab or NAS node. The node is
the cloud: agents report in over a LAN or tailnet, and the human watches
everything from an installable PWA.

This is an Open Knowledge Format v0.2 bundle: a human- and agent-readable wiki
covering the project's public design, architecture, decisions, and
contribution guidance. It is maintained to reflect status quo as the project
evolves. The working specification is held outside this bundle.

The hub is an early work in progress: the core surface ships (the feed,
session brains, the inbox, artifacts, search, and per-agent identity), and
some features remain intended design. Each page says which it is.

## Getting started

* [Overview](overview.md) - what Agent Hub is and why it exists
* [Contributor guide](contribution/guide.md) - how to contribute

## Usage

* [Usage overview](usage/index.md) - running the hub
* [Quickstart](usage/quickstart.md) - build, run, and configure a local instance

## Design

* [Design overview](design/index.md) - the approach in one place
* [Goals and non-goals](design/goals.md) - what v1 does and does not cover
* [Terminology](design/terminology.md) - the canonical vocabulary
* [Human interface](design/human-interface.md) - the design system and rules

## Architecture

* [Architecture overview](architecture/index.md) - components and layout
* [Components](architecture/components.md) - the process model and subsystem
  boundaries
* [Data model](architecture/data-model.md) - the hub store and the session
  brain file
* [Agent surface](architecture/agent-surface.md) - the MCP tools
* [Human surface](architecture/human-surface.md) - the REST API and PWA

## Decisions

* [Architecture decision records](adr/index.md) - the decisions that bind the
  design

## Contribution

* [Contribution overview](contribution/index.md) - guides for contributors
  and maintainers

## Repository contract

`AGENTS.md` holds the operational contract for humans and agents. This wiki
never contains internal names, codenames, hostnames, absolute paths, tokens,
or task identifiers, and the project's internal progress log is not part of
it.

## Knowledge base

* [Documentation log](log.md) - how this wiki has evolved
