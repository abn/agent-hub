---
okf_version: "0.2"
---

# Agent Hub documentation

Agent Hub is a local-first operations layer for a fleet of AI agents and their
human, shipped as one lean binary or container on a homelab or NAS node. The
node is the cloud: agents report in over a LAN or a tailnet, and the human
watches everything from an installable app.

This bundle is the whole guide, in two tracks: running the hub and working with
your agents, and building and maintaining the project. Decide where you are,
then follow one link.

## Which are you?

| You are | Start here |
|---|---|
| Looking at screenshots first | [Product tour](screens.md) |
| New, and want it running in five minutes | [Try it in five minutes](start/try-it.md) |
| Running it on your own node, any way you like | [Choose your setup](start/choose-your-setup.md) |
| Connecting an agent you already have | [Connect your first agent](start/connect-first-agent.md) |
| Already running it, and stuck on a task | [Guides](guides/index.md) |
| Running it day to day | [Operate](operate/index.md) |
| Looking up a setting, a tool, or a word | [Reference](reference/index.md) |
| Building on or changing the project | [Contributing](contribute/index.md) |

## What Agent Hub is

One process on your node gives your agents three things they otherwise lose and
gives you one calm place to watch them:

- **Session brains.** Every agent session keeps its working state on the node,
  not in the client's context window, so it survives a compaction, a restart, or
  a move to another machine.
- **A project feed.** "What happened in this project" is a first-class,
  time-ordered, addressable query. Agents read it on startup; you read it as a
  feed.
- **A place for you.** A global inbox for finished work and approvals, artifact
  hosting with browser-side encryption, search, and a wiki over each project's
  durable knowledge, all reachable from an installable app.

There is no vendor cloud and no brain off the node. Agents reach the hub over
the Model Context Protocol (MCP); you reach it through the app.

> [!NOTE]
> The hub is a work in progress. The feed, session brains, the knowledge base
> and its wiki, the inbox, artifacts, search, and per-agent identity ship today;
> a few things are intended design. Each page says which it is, and
> [Overview](overview.md) draws the line once.

## The sections

* [Overview](overview.md) - What Agent Hub is, why it exists, and what ships today
* [Product tour](screens.md) - Every screen of the human surface, at both widths in both themes
* [Get started](start/index.md) - install it, log in, and connect the first
  agent, whatever your setup.
* [Guides](guides/index.md) - the tasks: the human's and the agent's.
* [Operate](operate/index.md) - deploy it as a service, back it up, upgrade it.
* [Reference](reference/index.md) - settings, commands, tools, routes, words.
* [Architecture](architecture/index.md) - how it is built and why.
* [Design](design/index.md) - the interface foundation.
* [Contributing](contribute/index.md) - workflow, tests, the docs standard.
* [Decisions](adr/index.md) - the architecture decision records.

## Repository contract

`AGENTS.md` holds the operational contract for humans and agents. This bundle
never contains internal names, codenames, hostnames, absolute paths, tokens, or
task identifiers. The working specification is held outside the committed tree.

* [Documentation update log](log.md) - how this bundle has evolved.
