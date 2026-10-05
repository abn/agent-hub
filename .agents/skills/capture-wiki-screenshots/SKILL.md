---
name: capture-wiki-screenshots
description: Capture or refresh the Agent Hub wiki's product screenshots (mobile and desktop, dark and light) from a seeded scratch hub with dummy data, and wire them into the OKF docs bundle. Use when adding screenshots to the wiki, when a screen changed and its screenshot is stale, when asked to update the docs images, or when a review finds the wiki has no visuals.
---

# Capture and update the wiki screenshots

The Agent Hub wiki under `docs/` is an OKF v0.2 bundle that the shared wiki
renders directly (every non-Markdown file in the bundle is emitted at the same
path). Product screenshots live at `docs/assets/screens/<route>-<width>-<theme>.png`
and are embedded from the pages that describe the screen.

The images are a committed artifact, so they carry the project's rules: real
dummy data, no secrets, no hostnames, no internal identifiers, no emoji, and a
consistent size. They are not marketing shots; they are what a reader checks a
claim against.

## What good looks like

- A written capture script a reviewer can re-run, not a folder of mystery
  bytes: `.agents/skills/capture-wiki-screenshots/scripts/wiki_screens.py`.
- Four images per feature screen: desktop light and dark at 1440x900, phone
  light and dark at 390x844, so a reader on either theme at either width sees
  the real surface.
- Dummy data from `.agents/skills/seeded-hub/scripts/hub_harness.py` (`running_hub(name)`,
  `seed(port)`), which seeds a `checks` project with a feed, a session with a
  brain, artifacts, an inbox and a knowledge base. Never a real hub, never the
  live `data/`, never a real token.
- A deterministic run: fixed viewport, fixed seed, reduced motion forced off
  where it would blank a transition, and the same filenames each time so a
  refresh is a byte-level diff rather than a new set.

## The capture script

`.agents/skills/capture-wiki-screenshots/scripts/wiki_screens.py` is the single source of truth for the set.
It:

1. Starts a seeded hub through `hub_harness.running_hub`.
2. Uses `hub_harness.launch_browser` and, for each (route, width, theme),
   sets `data-theme`, `data-density` and the viewport, navigates, waits for the
   screen's own selector, and screenshots to `docs/assets/screens/`.
3. Runs `harness.router_screens()` and captures the main feature screens; a
   screen added to the router with no capture is a gap the script reports.

Screens worth carrying (the main features, not every route): `home`, `inbox`
(and an open item), `projects`, `feed`, `sessions` and the session detail with
the brain tree, `artifacts` and the artifact viewer, `wiki` and a reader page,
`search`, `storage`, `settings`, `access`, and `connect`.

Run it with the interpreter that can import playwright:

```sh
python3 .agents/skills/capture-wiki-screenshots/scripts/wiki_screens.py    # the interpreter that can import playwright
```

It writes into a bundle that is checked in, so it must be run from the repo
root and it must not touch anything outside `docs/assets/screens/`.

### Which browser it drives

The script launches a bundled Chromium by default. It drives **Obscura** over
CDP instead when `OBSCURA_CDP` names an endpoint that answers as Obscura, so a
capture need not contend for a Chromium with another agent on the machine:

```sh
OBSCURA_ALLOW_PRIVATE_NETWORK=1 obscura serve --port 9222 &
OBSCURA_CDP=http://127.0.0.1:9222 python3 .agents/skills/capture-wiki-screenshots/scripts/wiki_screens.py
```

Obscura needs `OBSCURA_ALLOW_PRIVATE_NETWORK=1` (its server flag) for the
scratch hub on loopback. Over CDP it exposes one context and no
`Target.createTarget`, and it does **not** keep `localStorage` across pages, so
the token must be seeded by an init script that runs before the app module
reads it, on a page whose viewport is set before the first navigation. The
script does exactly that; a capture that renders the chrome but no data is the
sign the token was seeded too late. The resulting set is byte-for-byte close to
the Chromium set, so either browser is acceptable.


## Wiring an image into a page

A screenshot is never one edit. In the same commit:

1. The captures under `docs/assets/screens/`.
2. The page that describes the screen gains a figure with a caption and alt
   text, for example:

   ```markdown
   ![The inbox at desktop width with two approvals waiting.](../assets/screens/inbox-1440-light.png)
   ```

   In the architecture and design pages use bundle-absolute links
   (`/assets/screens/...`) so the figure survives a page moving.
3. `docs/log.md` gets one line (a bare `YYYY-MM-DD` heading, the entry title as
   a `###` subheading), naming the screens captured.

Alt text says what is on the screen, not "screenshot of the inbox". A caption
is optional; the surrounding prose usually carries it.

## Keeping it up to date

The screenshots are stale the moment a screen changes. Three hooks keep them
honest:

- `DESIGN.md` and `AGENTS.md` already make looking at 390 and 1440 a build
  expectation; the capture script is how that look is recorded.
- The design-conformance and review rounds re-run the script and commit the
  fresh set as part of the change, not after it.
- `docs/log.md`'s line names what moved, so a stale figure is traceable to the
  change that should have refreshed it.

When a screen's layout changes, re-run the script, look at the four images for
that screen, and commit them with the change. When only the seeded data
changes, re-run the whole set so the bundle stays consistent.

## Rules

- Dummy data only. The seeded hub carries no real token and no live store.
- The scratch hub **must** set a fixed `HUB_NODE_NAME` (the harness sets
  `HUB_NODE_NAME=local`). Without it the header renders the machine's own
  hostname, which the always-public rule forbids in a committed bundle. This
  was caught in the first capture: every header read the host's real name until
  the harness set one.
- The scratch hub **must** use a deployment-shaped data directory. Settings
  shows the hub's data path, so the default scratch directory put an absolute
  home path in a public bundle. The capture passes a relative `data`, which
  renders as exactly that; a path that leaks a home or a build tree is a defect
  the text-scanning hooks cannot catch, because a PNG is not text.
- Seed the token with an init script, never with `page.evaluate` after the
  first load: the app reads `localStorage` once at module load (`web/prefs.mjs`),
  so a token written later is invisible and every data screen renders empty.
- No PII, no hostnames, no tailnet names, no `.agents/brain` paths.
- No emoji in a filename or a caption. No em-dashes in prose.
- The set stays small: the main feature screens, both widths, both themes.
  A screen no page describes does not need a capture.
- The bundle is public the moment it is committed. Look at each image before
  it lands.
