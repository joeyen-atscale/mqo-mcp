#!/usr/bin/env bash
# audit.sh — BAD_RUST scan wrapper.
# READ-ONLY: the edit-agent must not modify this file.
set -euo pipefail
cd "$(dirname "$0")/.."

exec "$HOME/.claude/skills/autobuilder/rules/audit-checks.sh" .
