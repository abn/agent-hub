---
type: Guide
title: Notifications
description: Nudge a closed app through a notification target you run
tags: [operator, notifications, config]
status: stable
---

# Notifications

By the end you will have the hub POSTing a contentless nudge to a target you
run whenever something starts waiting on you. The interactive alert model is in
[Human interface](/design/human-interface.md).

## Before you start

| Thing | Why |
|---|---|
| A notification service you run | the hub sends to a URL; it does not host one |
| The admin token and the data directory | the setting lives in `config.toml` or the environment |

## Why there is no push

An installed app that is fully closed raises nothing on its own. Background push
notifications are deferred ([ADR 0016](/adr/0016-push-notifications-deferred.md)):
while the app is open it reads a freshness stream and refreshes its waiting
badge, with a slow poll as the fallback, but with the app fully closed nothing
is raised.

The one exception is opt-in. Point the hub at a notification service you run,
such as a self-hosted ntfy topic on the LAN, and it sends a contentless nudge
when a question, an approval, or an enrolment request starts waiting
([ADR 0026](/adr/0026-contentless-notify-target.md)). With no target set,
nothing is sent.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/screens/settings-desktop-dark.png">
  <img src="../assets/screens/settings-desktop-light.png" alt="Settings, its rows under quiet group labels.">
</picture>

## Set a target

Set `notify_url` to an `http` or `https` URL that accepts a plain-text POST:

```toml
[hub]
notify_url = "https://ntfy.lan/agent-hub?title=Agent+Hub&click=https://hub.lan"
notify_token = "tk_..."
notify_interval_secs = 60
```

The equivalent environment variables are `HUB_NOTIFY_URL`,
`HUB_NOTIFY_TOKEN`, and `HUB_NOTIFY_INTERVAL_SECS`. The setting is off by
default and nothing is sent without it.

> [!IMPORTANT]
> `notify_url` may not carry credentials in the URL; use `notify_token` instead.
> The token is sent only in the `Authorization` header and is never logged or
> printed, and `agent-hub config` shows the URL's origin with any path or query
> hidden.

ntfy reads its own message options from the query, so the title and the tap
target are set there, by you; the hub sends neither. Subscribe to the topic in
the ntfy app on the phone. A target that wants another shape, such as Gotify's
JSON message, needs a small bridge in front of it.

## What is sent

The hub POSTs one fixed sentence to the target:

```
Something is waiting for you in Agent Hub.
```

That sentence is the whole payload. It names no project, agent, title, id, or
count, so the target learns only that it is worth opening the app.

Sends are coalesced: at most one per `notify_interval_secs`, and anything that
arrives inside the quiet period is one trailing send when it ends. A send runs
in the background with a ten-second timeout and is not retried, so a slow or
unreachable target never holds up the agent's write. A failure is logged at warn
with the target's origin and the cause, and counted in
`agenthub_notify_sends_total{result="failed"}`. A retried write that the hub
answers from its idempotency key posts nothing new, so it sends nothing.

## You should now see

With the app closed, a question, an approval, or an enrolment request that
starts waiting reaches your target as the one sentence above, and
`GET /metrics` shows `agenthub_notify_sends_total{result="delivered"}` moving.

## See also

- [Monitoring](/operate/monitoring.md) - the notify counter and its failed result
- [Configuration](/reference/configuration.md) - `notify_url`, `notify_token`, `notify_interval_secs`
- [Human interface](/design/human-interface.md) - the in-app alert states
