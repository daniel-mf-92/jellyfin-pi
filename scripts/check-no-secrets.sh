#!/usr/bin/env bash
set -euo pipefail

mode="${1:---staged}"
if [[ "$mode" != "--staged" && "$mode" != "--full" ]]; then
  echo "Usage: $0 [--staged|--full]" >&2
  exit 2
fi

# Personal/private infra markers + high-risk credential signatures.
# NOTE: keep this list focused to reduce false positives.
pattern='10\.100\.0\.[0-9]+|your-username|your-github-username|@garden-stack\.com|azureuser@|AKIA[0-9A-Z]{16}|ghp_[0-9A-Za-z]{36}|github_pat_[0-9A-Za-z_]{20,}|sk_(live|test)_[0-9A-Za-z]{16,}|xox[baprs]-[0-9A-Za-z-]{10,}|-----BEGIN (RSA|EC|OPENSSH|DSA|PGP) PRIVATE KEY-----'

check_env_files() {
  local files
  if [[ "$mode" == "--staged" ]]; then
    files=$(git diff --cached --name-only --diff-filter=ACMR)
  else
    files=$(git ls-files)
  fi

  local bad=()
  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    base="$(basename "$file")"
    if [[ "$base" == ".env" ]]; then
      bad+=("$file")
      continue
    fi
    if [[ "$base" == .env.* && "$base" != ".env.example" ]]; then
      bad+=("$file")
      continue
    fi
  done <<< "$files"

  if (( ${#bad[@]} > 0 )); then
    echo "❌ Commit blocked: environment secret files are staged/tracked:" >&2
    printf '  - %s\n' "${bad[@]}" >&2
    echo "Keep real secrets in local .env only (gitignored), and only commit .env.example placeholders." >&2
    return 1
  fi
}

scan_staged() {
  local diff matches
  diff=$(git diff --cached --no-color -U0 -- . \
    ':(exclude)scripts/check-no-secrets.sh' \
    ':(exclude).githooks/pre-commit' \
    ':(exclude).githooks/pre-push')

  matches=$(printf '%s\n' "$diff" \
    | grep -vE '^\+\+\+' \
    | rg -n -i "^\+.*($pattern)" || true)

  if [[ -n "$matches" ]]; then
    echo "❌ Commit blocked: hardcoded personal/secret markers detected in staged changes:" >&2
    echo "$matches" >&2
    echo "Use env vars / placeholders instead of hardcoded values." >&2
    return 1
  fi
}

scan_full() {
  local matches
  matches=$(git ls-files -z \
    | xargs -0 rg -n --no-heading -i "$pattern" \
      --glob '!scripts/check-no-secrets.sh' \
      --glob '!.githooks/pre-commit' \
      --glob '!.githooks/pre-push' || true)

  if [[ -n "$matches" ]]; then
    echo "❌ Push blocked: repository contains hardcoded personal/secret markers:" >&2
    echo "$matches" >&2
    echo "Replace with env vars/placeholders before pushing." >&2
    return 1
  fi
}

check_env_files
if [[ "$mode" == "--staged" ]]; then
  scan_staged
else
  scan_full
fi

echo "✅ Secret guard passed ($mode)."
