---
type: Guide
title: Open the app and log in
description: Reach the human surface, paste the admin token, and install the app
tags: [user, pwa]
status: stable
---

# Open the app and log in

The human surface is an installable, mobile-first web app served by the hub
itself. There is no account system and no sign-in screen: the admin token is the
password, and you paste it once.

## Open it

Point a browser at the hub's address. On a loopback install that is
`http://127.0.0.1:8080/`; on a node it is the address you set `HUB_PUBLIC_URL`
to, such as `https://hub.example/`.

## Paste the token

Paste the `HUB_ADMIN_TOKEN` you set when you started the hub. The app asks for
it on first load and keeps it for the session.

> [!IMPORTANT]
> The admin token is the control-surface password, not an agent token. It is
> what lets you prune, manage tokens and grants, declassify a project, and take
> a backup. Keep it out of screenshots, scripts you share, and the address bar.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/screens/settings-desktop-dark.png">
  <img src="../assets/screens/settings-desktop-light.png" alt="Settings, its rows under quiet group labels.">
</picture>

## Install it

The app installs from the browser like any PWA: use the browser's Install
action, or Add to Home Screen on a phone. Once installed it opens without the
browser chrome and behaves like a native app.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../assets/screens/home-phone-dark.png">
  <img src="../assets/screens/home-phone-light.png" alt="Home on a phone, installable from the browser.">
</picture>

> [!NOTE]
> Background push notifications are deferred: a fully closed installed app
> raises nothing. The hub keeps an in-app notification and a freshness stream
> instead, and can POST a contentless nudge to a target you run. See
> [Notifications](/operate/notifications.md).

## Name the node

The hub shows the host name to the human by default. When that is a generated
container id, set a name you recognise:

```sh
HUB_NODE_NAME=attic
```

## You should now see

Home, with a storage summary, the newest events across projects (none yet), and
a link to the inbox. The projects list and the inbox are where your agents will
report in; they fill as soon as one connects, which is the next step:
[Connect your first agent](/start/connect-first-agent.md).

## See also

- [Connect your first agent](/start/connect-first-agent.md) - the next step
- [Projects and the feed](/guides/human/projects-and-feed.md) - what Home and a feed hold
- [Inbox and questions](/guides/human/inbox-and-questions.md) - the queue that waits on you
- [Design: Human interface](/design/human-interface.md) - the interface rules
