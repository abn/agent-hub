#!/usr/bin/env bash
# Idempotent repository bootstrap. Safe to re-run at any time.
#
# Installs the declarative git hooks and writes the git-excluded assistant
# shims, so the committed tree stays free of tool-specific files.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

chmod +x .agents/bootstrap.sh .agents/scripts/*.py

# Assistant shims point at the one committed contract. Listed in .gitignore.
for shim in CLAUDE.md GEMINI.md; do
  printf 'Read AGENTS.md and follow it.\n' > "$shim"
done

mkdir -p .agents/brain/{inbox,outbox,tasks,assets}

if ! command -v pre-commit >/dev/null 2>&1; then
  printf 'bootstrap: hooks NOT installed: pre-commit is missing.\n' >&2
  printf 'bootstrap: shims written and scratch area ready.\n' >&2
  printf 'bootstrap: install pre-commit (for example "pipx install\n' >&2
  printf 'bootstrap: pre-commit") and re-run this script.\n' >&2
  exit 1
fi

pre-commit install --install-hooks
printf 'bootstrap: hooks installed, shims written, scratch area ready\n'
