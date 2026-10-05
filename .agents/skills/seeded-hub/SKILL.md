---
name: seeded-hub
description: Start a throwaway Agent Hub seeded with dummy data, for the Playwright browser checks and the wiki screenshot capture. Use when a browser test or a screenshot run needs a running hub with a known project, feed, session brain, artifacts, inbox and knowledge base, without touching a real store.
---

# Run a seeded hub

`scripts/hub_harness.py` starts one Agent Hub process against a throwaway data
directory and seeds it with a known fixture, then hands back the port and the
seeded ids. It is test infrastructure: the Playwright suite
(`e2e/hub-bridge.py`) and the wiki screenshot capture
(`capture-wiki-screenshots`) both drive it, and nothing else should.

The hub it starts is not a real one. It uses a scratch data directory, a
generated admin token, and `HUB_NODE_NAME=local` so the header never renders the
machine's hostname. It never opens the live `data/` and never touches a real
store.

## Use it

```python
import hub_harness as harness

with harness.running_hub("my-check") as (port, seeded):
    # seeded["project_id"], seeded["artifact_id"], seeded["session_id"], ...
    ...
```

`running_hub(name)` yields the bound port and the seeded ids, and tears the hub
and its data directory down on exit. It is a context manager on purpose: a
killed run must not leave a process or a data directory behind.

Other helpers: `seed(port)` seeds a hub already running, `launch_browser(playwright, name)`
returns a Chromium, and `router_screens()` lists the screens a capture should
cover.

## Rules

- Dummy data only. The seeded project is `checks`; there is no real token and no
  live store.
- A fixed `HUB_NODE_NAME=local`. Without it the header renders the machine's own
  hostname, which a committed screenshot would leak.
- A deployment-shaped data directory, passed as a relative `data`, so a
  screenshot of Settings shows a path a deployment would, not a home directory.
- The harness is the only writer of its data directory. Never point it at a
  directory a hub is already serving.

## Where it is used

- `e2e/hub-bridge.py` seeds one hub per Playwright project and writes the
  descriptor the specs read.
- `capture-wiki-screenshots/scripts/wiki_screens.py` seeds a hub and photographs
  the feature screens from it.
