#!/usr/bin/env bash
# risk-gate.sh — checks receipts are present and gate conditions pass.
# READ-ONLY: the edit-agent must not modify this file.
set -euo pipefail
cd "$(dirname "$0")/.."

exec "$HOME/.claude/skills/autobuilder/scripts/risk-gate.sh" .
