---
type: Decision Record
title: Browser-side artifact encryption
description: Protected artifacts are encrypted in the browser; the server stores ciphertext only.
tags: [adr, security, artifacts, e2ee]
status: stable
---

# 0009. Browser-side artifact encryption

## Context

Artifacts are meant to be shareable, including with people outside the
tailnet. Sharing a link through the hub should not require the server to hold
the plaintext of anything sensitive, and it should not require the recipient
to install anything.

## Decision

Protected artifacts are encrypted in the browser before upload, with a random
key per artifact. The server stores ciphertext and the envelope and never
sees plaintext. A recipient opens the link, enters the password, and
decryption happens in the browser.

## Consequences

- An artifact is public only when its author explicitly publishes it without
  protection.
- The server cannot search protected artifact content, only its metadata.
- Sharing a protected artifact requires a password out of band, which is the
  intended cost.
- Anything leaving the machine, in a shared artifact or a bug report, follows
  the redaction-by-default rule from the global standards, and outward
  actions are confirmed with the human first.
