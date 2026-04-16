#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="${REPO_DIR:-$HOME/Documents/local-codebases/Pi-Media-Player}"
PROMPT_FILE="${PROMPT_FILE:-$REPO_DIR/automation/LOOP_PROMPT.md}"
LOG_DIR="${LOG_DIR:-$REPO_DIR/automation/logs}"
SLEEP_SECONDS="${SLEEP_SECONDS:-180}"
BRANCH_NAME="${BRANCH_NAME:-slint-rewrite}"
CODEX_TIMEOUT_SECONDS="${CODEX_TIMEOUT_SECONDS:-1200}"
CODEX_KILL_AFTER_SECONDS="${CODEX_KILL_AFTER_SECONDS:-30}"
CODEX_MAX_RETRIES="${CODEX_MAX_RETRIES:-2}"
CODEX_RETRY_DELAY_SECONDS="${CODEX_RETRY_DELAY_SECONDS:-30}"
LOCK_DIR="${LOCK_DIR:-$REPO_DIR/automation/.codex-loop.lock}"
LOCK_PID_FILE="$LOCK_DIR/pid"
HEARTBEAT_FILE="${HEARTBEAT_FILE:-$LOG_DIR/loop.heartbeat}"
CREDENTIALS_FILE="${CREDENTIALS_FILE:-$HOME/.mcp-credentials.env}"

# Load balancer endpoint files (reuse TempleOS Codex 5.3 endpoints)
LB_ENDPOINT_FILES=(
  "$HOME/.codex/codex53-endpoints.json"
  "$HOME/.codex/codex53-2-endpoints.json"
  "$HOME/.codex/codex53-3-endpoints.json"
  "$HOME/.codex/codex53-4-endpoints.json"
)

mkdir -p "$LOG_DIR"

[[ -f "$PROMPT_FILE" ]] || { echo "Missing prompt: $PROMPT_FILE"; exit 1; }
[[ -f "$CREDENTIALS_FILE" ]] && source "$CREDENTIALS_FILE"

# --- Lock ---
cleanup() { rm -rf "$LOCK_DIR" 2>/dev/null; }
trap cleanup EXIT
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  OLD_PID=$(cat "$LOCK_PID_FILE" 2>/dev/null || echo "")
  if [[ -n "$OLD_PID" ]] && kill -0 "$OLD_PID" 2>/dev/null; then
    echo "Another loop running (PID $OLD_PID), exiting."
    exit 0
  fi
  rm -rf "$LOCK_DIR"
  mkdir "$LOCK_DIR"
fi
echo $$ > "$LOCK_PID_FILE"

# --- Load balancer: pick a random endpoint ---
pick_endpoint() {
  local files=()
  for f in "${LB_ENDPOINT_FILES[@]}"; do
    [[ -f "$f" ]] && files+=("$f")
  done
  [[ ${#files[@]} -eq 0 ]] && return 1
  local file="${files[$((RANDOM % ${#files[@]}))]}"

  # Files contain JSON array of endpoints - pick random one
  local py_out
  py_out=$(python3 -c "
import json, random, sys, shlex
d = json.load(open(sys.argv[1]))
e = random.choice(d) if isinstance(d, list) else d
print(e['base_url'])
print(e['api_key'])
print(e.get('model', 'gpt-53-codex'))
" "$file" 2>/dev/null) || return 1

  LB_BASE_URL=$(echo "$py_out" | sed -n '1p')
  LB_API_KEY=$(echo "$py_out" | sed -n '2p')
  LB_MODEL=$(echo "$py_out" | sed -n '3p')

  export CODEX_API_KEY="$LB_API_KEY"
  export AZURE_OPENAI_API_KEY="$LB_API_KEY"
}

# --- Main loop ---
ITERATION=0
while true; do
  ITERATION=$((ITERATION + 1))
  TS=$(date +%Y%m%d-%H%M%S)
  echo "$TS iteration=$ITERATION" > "$HEARTBEAT_FILE"

  cd "$REPO_DIR"
  git checkout "$BRANCH_NAME" 2>/dev/null || true
  git pull --rebase origin "$BRANCH_NAME" 2>/dev/null || true

  PROMPT=$(cat "$PROMPT_FILE")

  # Append Pi log context
  PI_LOG=$(ssh -o ConnectTimeout=5 -o BatchMode=yes danielmatthews-ferrero@10.100.0.17 \
    "tail -30 /tmp/jmp-slint.log 2>/dev/null" 2>/dev/null || echo "(Pi unreachable)")
  PROMPT="$PROMPT

## Current Pi Log (last 30 lines)
\`\`\`
$PI_LOG
\`\`\`

## Current Iteration
Iteration $ITERATION at $TS. Fix the highest-priority issue you can identify from the code and logs."

  # Pick endpoint
  if pick_endpoint; then
    CODEX_ARGS=(
      -a never -s workspace-write exec --json
      --model "$LB_MODEL"
      -c "model_providers.azure.base_url=$LB_BASE_URL"
      -c "model_providers.azure.wire_api=responses"
      -c "model_providers.azure.timeout=$CODEX_TIMEOUT_SECONDS"
      -c "model_providers.azure.stream_idle_timeout_ms=3600000"
      -c "model_providers.azure.request_max_retries=10"
      -c "model_providers.azure.stream_max_retries=8"
      --cd "$REPO_DIR"
      --skip-git-repo-check
      --output-last-message "$LOG_DIR/$TS.final.txt"
      "$PROMPT"
    )
  else
    echo "[$TS] No endpoints available, sleeping..."
    sleep "$SLEEP_SECONDS"
    continue
  fi

  echo "[$TS] Starting iteration $ITERATION..."

  RETRY=0
  while [[ $RETRY -lt $CODEX_MAX_RETRIES ]]; do
    if timeout "${CODEX_TIMEOUT_SECONDS}s" codex "${CODEX_ARGS[@]}" > "$LOG_DIR/$TS.out.log" 2>&1; then
      echo "[$TS] Iteration $ITERATION completed successfully."
      break
    else
      EXIT_CODE=$?
      RETRY=$((RETRY + 1))
      echo "[$TS] Codex exited $EXIT_CODE (retry $RETRY/$CODEX_MAX_RETRIES)"
      sleep "$CODEX_RETRY_DELAY_SECONDS"
    fi
  done

  # Push any changes
  if [[ -n "$(git status --porcelain)" ]]; then
    git add -A
    git commit -m "codex: pi-media-player iteration $ITERATION auto-fix ($TS)" --no-verify 2>/dev/null || true
    git push origin "$BRANCH_NAME" 2>/dev/null || true
    echo "[$TS] Pushed changes from iteration $ITERATION"
  fi

  echo "[$TS] Sleeping ${SLEEP_SECONDS}s..."
  sleep "$SLEEP_SECONDS"
done
