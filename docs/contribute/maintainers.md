---
type: Guide
title: Maintainer guide
description: The review gate and the ground-truth rules for the wiki
tags: [contributor, contribution]
status: stable
---

# Maintainer guide

Every change passes one skeptical review before it is pushed. This page states
the gate and the ground truth the wiki is held to. The contributor's side of the
same process is in the [Contributor guide](/contribute/guide.md).

## The review gate

A change is read as a skeptical maintainer before push. An agent session follows
the reviewer role card under `.agents/agents/`; a person applies the same
criteria:

- Does the change satisfy its stated scope, and is it the smallest clean change
  that does so?
- Does it respect the invariants: one engine, wrap AgentFS rather than
  reimplement it, one writer per session file, no automatic expiry, no chat, and
  engine-native search?
- Is every committed file and commit message free of internal process,
  tracking, task, or scratch-area references?
- Do the tests assert real behaviour, and does the wiki reflect what the code
  now does? Was `docs/log.md` updated for the wiki changes?
- Could the change break an existing agent client or the human surface?

A review returns either a pass or a list of concrete issues with a proposed fix.
The human holds the final decision to push.

## Ground truth

The wiki reflects the status quo. When the code and a page disagree, the code
wins, and the correction is recorded in [the log](/log.md). A page that describes
an intended design says so in plain words, so a reader never mistakes a plan for
a shipped feature.

The internal progress log is not part of the wiki. It lives in the gitignored
scratch area and is never committed.

## See also

- [Contributor guide](/contribute/guide.md) - the workflow and the gates
- [Testing](/contribute/testing.md) - the checks a review reads
- [Documentation](/contribute/documentation.md) - the docs standard
