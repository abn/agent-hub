---
type: Guide
title: Maintainer guide
description: The review gate, ground truth, and keeping the wiki honest.
tags: [contribution, review, maintenance]
status: draft
---

# Maintainer guide

## The review gate

Every change is reviewed as a skeptical maintainer before it is pushed. The
role card in `.agents/agents/reviewer.md` describes the gate for an agent
session; the criteria are the same for a person.

- Does the change satisfy its stated scope, and is it the smallest clean
  change that does so?
- Does it respect the invariants: one engine, wrap AgentFS rather than
  reimplement it, one writer per session file, no automatic expiry, no chat,
  and engine-native search?
- Is every committed file and commit message free of internal process,
  tracking, task, or scratch-area references?
- Do the tests assert real behaviour, and does the wiki reflect what the code
  now does? Was `docs/log.md` updated for wiki changes?

A review returns either a pass or a list of concrete issues with a proposed
fix. The human holds the final decision to push.

## Ground truth

The wiki reflects status quo. When the code and a page disagree, the code
wins, and the correction is recorded in [the log](../log.md). Pages that
describe an intended design say so in plain words, so a reader never mistakes
a plan for a shipped feature.

The internal progress log is not part of the wiki. It lives in the gitignored
scratch area.
