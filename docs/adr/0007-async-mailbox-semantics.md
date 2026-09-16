---
type: Decision Record
title: Asynchronous mailbox semantics
description: Human and agent interaction is asynchronous mailboxes, with no real-time chat in v1.
tags: [adr, interaction, inbox]
status: stable
---

# 0007. Asynchronous mailbox semantics

## Context

A chat interface between the human and agents is the expected shape, and it
is also a trap: it demands presence, invites scope creep into a conversational
product, and pulls the human back to a screen to babysit agents.

## Decision

Interaction is asynchronous, with mailbox semantics. Agents post questions
and signals; the human reads, answers, approves, and prunes when it suits
them. There is no real-time chat with agents in v1.

## Consequences

- Questions from agents land in the inbox and the feed, and answers land back
  in the feed and mark the question acted on.
- The PWA notification channel, not an open window, is the signal that
  something needs the human.
- The human surface stays calm and bounded, which keeps it shippable.
