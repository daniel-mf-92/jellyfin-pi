#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
chmod +x scripts/check-no-secrets.sh .githooks/pre-commit .githooks/pre-push

git config core.hooksPath .githooks
echo "✅ Installed repo git hooks (core.hooksPath=.githooks)."
echo "pre-commit: scans staged changes for personal/secret markers"
echo "pre-push: scans full tracked tree before push"
