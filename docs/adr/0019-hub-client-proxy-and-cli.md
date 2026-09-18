---
type: Decision Record
title: The hub client is a proxy and a CLI
description: stdio MCP is a proxy to the one running hub, a small CLI serves harness hooks, and both read the same env-style settings.
tags: [adr, mcp, client, configuration, hooks]
status: stable
---

# 0019. The hub client is a proxy and a CLI

## Context

Two callers could not reach a running hub. A harness that speaks only stdio
MCP had to run `agent-hub mcp`, which opened the data directory itself; the
engine holds that directory exclusively, so the command failed whenever a hub
was already serving it, and it could never work from another machine at all.
A harness hook is a shell command with no MCP client, so it had no way in
even over HTTP.

Both are the same gap: the node is the cloud, agents reach it over LAN or
tailnet, and the client side of that promise was missing.

## Decision

stdio is a proxy. With a hub URL configured, `agent-hub mcp` holds one
streamable HTTP connection to the hub for the life of the process and forwards
every request to it, so the tools, their errors, and the identity are the
hub's own, and a tool the hub gains needs no new client. One process is one
connection, which is what keeps the hub's per-connection active session usable
through the proxy. With no URL configured the command still serves the local
data directory standalone, as the human admin, and says so: the same command
gives a harness admin rights or one agent's rights depending on that setting,
so the mode is named on startup.

The CLI exists for hooks. `agent-hub call <tool> [json]` makes one call and
prints the tool's JSON result on stdout and nothing else; the hub's own error
object goes to stderr, and the exit code follows `sysexits.h` so a hook can
tell a down hub (69) from a refused token (77) from a missing setting (78)
without parsing text. A call is its own connection and holds no session.

Settings are an env-style file. `~/.agent-hub/config` carries `HUB_URL`,
`HUB_TOKEN`, and `HUB_AGENT_ID`, the same names as the environment, parsed by
hand; the environment wins key by key. One format for both means a hook can
source the file or export the variables and behave identically, and three
scalars do not earn a configuration-format dependency. A token file others can
read warns and still works, because refusing would break a working setup on a
machine the operator already controls.

The client sits behind a `client` cargo feature, on by default. The container
image, which only serves, builds without it.

## Consequences

- One hub serves every agent on every node, and a harness that speaks only
  stdio is no longer confined to the machine holding the data directory.
- The embedded standalone mode stays for now because the quickstart documents
  it, at the cost of one command with two identities.
- The transport is rmcp's streamable HTTP client over `reqwest` with rustls,
  which adds four compiled crates and about 5 MB, roughly a tenth, to the
  release binary. That is a real cost against the lean-binary goal, accepted
  because the serve-only build the container uses does not pay it.
  Implementing rmcp's client trait by hand would have been
  roughly six hundred lines of HTTP and SSE machinery to own instead. Never
  `native-tls`: an OpenSSL linkage would break the distroless image.
- Hook shorthands over the project store are not part of this decision; they
  follow the store.
