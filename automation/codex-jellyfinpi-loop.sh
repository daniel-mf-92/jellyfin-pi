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
WATCHDOG_INTERVAL_SECONDS="${WATCHDOG_INTERVAL_SECONDS:-5}"
ALLOW_PI_DEPLOY="${ALLOW_PI_DEPLOY:-0}"

# Resource guardrails (prevent runaway RAM/CPU usage)
CODEX_MAX_VMEM_KB="${CODEX_MAX_VMEM_KB:-6291456}"
CODEX_MAX_RSS_MB="${CODEX_MAX_RSS_MB:-3072}"
CODEX_MAX_CPU_PERCENT="${CODEX_MAX_CPU_PERCENT:-250}"
CODEX_MAX_CPU_HITS="${CODEX_MAX_CPU_HITS:-6}"
CODEX_NICE_LEVEL="${CODEX_NICE_LEVEL:-10}"
MIN_FREE_MEM_MB="${MIN_FREE_MEM_MB:-3072}"
MAX_LOAD_PER_CORE="${MAX_LOAD_PER_CORE:-2.50}"
RESOURCE_BACKOFF_SECONDS="${RESOURCE_BACKOFF_SECONDS:-300}"
MAX_CONCURRENT_CODEX_PROCS="${MAX_CONCURRENT_CODEX_PROCS:-4}"
MAX_CONCURRENT_REPO_CODEX_PROCS="${MAX_CONCURRENT_REPO_CODEX_PROCS:-1}"

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

SAFETY_OVERRIDE=$(cat <<'EOF'
## Runtime Safety Override (Critical)
This override supersedes conflicting instructions earlier in this prompt.

If `ALLOW_PI_DEPLOY=0`:
- Do not run SSH build/deploy commands on `10.100.0.17`.
- Do not run `cargo build --release` on Pi.
- Do not launch `jellyfin-pi` with nohup.
- Use local verification only (`cargo check`) and Pi log tail (read-only).

If `ALLOW_PI_DEPLOY=1`:
- Use this safe Pi build command only (single-job, memory-capped, lock, timeout):
  ```bash
  ssh danielmatthews-ferrero@10.100.0.17 "bash -lc 'set -euo pipefail; flock -n /tmp/pi-media-player-build.lock timeout 25m bash -lc \"cd ~/Pi-Media-Player && git pull origin slint-rewrite && source ~/.cargo/env && export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 && ulimit -Sv 1800000 && nice -n 19 cargo build --release -j 1\"'"
  ```
- Install to the service binary path and restart the managed user service only:
  ```bash
  ssh danielmatthews-ferrero@10.100.0.17 "bash -lc 'set -euo pipefail; echo 5991 | sudo -S install -m 0755 ~/Pi-Media-Player/target/release/pi-media-player /usr/local/bin/pi-media-player; systemctl --user restart pi-media-player.service; sleep 8; tail -n 120 /tmp/pi-media-player.log 2>/dev/null || tail -n 120 /tmp/jmp-slint.log 2>/dev/null'"
  ```
EOF
)

# --- Lock ---
cleanup() { rm -rf "$LOCK_DIR" 2>/dev/null; }
trap cleanup EXIT INT TERM
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

log_line() {
  echo "[$(date +%Y-%m-%dT%H:%M:%S%z)] $*"
}

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

get_cpu_cores() {
  case "$(uname -s)" in
    Darwin)
      sysctl -n hw.logicalcpu 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 1
      ;;
    *)
      command -v nproc >/dev/null 2>&1 && nproc || echo 1
      ;;
  esac
}

get_load1() {
  case "$(uname -s)" in
    Darwin)
      sysctl -n vm.loadavg 2>/dev/null | tr -d '{}' | awk '{print $1}'
      ;;
    *)
      awk '{print $1}' /proc/loadavg 2>/dev/null || echo 0
      ;;
  esac
}

get_free_mem_mb() {
  case "$(uname -s)" in
    Darwin)
      local page_size free_pages free_mb
      page_size=$(sysctl -n hw.pagesize 2>/dev/null || echo 4096)
      free_pages=$(vm_stat 2>/dev/null | awk '/Pages free|Pages speculative/ {gsub("\\.","",$3); sum += $3} END {print int(sum)}')
      [[ -z "$free_pages" ]] && free_pages=0
      free_mb=$(( (free_pages * page_size) / 1024 / 1024 ))
      echo "$free_mb"
      ;;
    *)
      awk '/MemAvailable:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 0
      ;;
  esac
}

count_codex_processes() {
  local count
  count=$(pgrep -x codex 2>/dev/null | wc -l | tr -d '[:space:]')
  [[ -z "$count" ]] && count=0
  echo "$count"
}

count_repo_codex_processes() {
  ps ax -o command= | awk -v repo="$REPO_DIR" '
    /codex / {
      if (index($0, "--cd " repo) > 0) c++
    }
    END {print c + 0}
  '
}

should_backoff_for_system_load() {
  local free_mem_mb load1 cores load_per_core

  free_mem_mb=$(get_free_mem_mb)
  load1=$(get_load1)
  cores=$(get_cpu_cores)
  [[ -z "$cores" || "$cores" -lt 1 ]] && cores=1

  load_per_core=$(awk -v load="$load1" -v cores="$cores" 'BEGIN {printf "%.2f", load / cores}')

  if [[ "$free_mem_mb" -lt "$MIN_FREE_MEM_MB" ]]; then
    log_line "Resource guard: low free memory ${free_mem_mb}MB (< ${MIN_FREE_MEM_MB}MB), backing off ${RESOURCE_BACKOFF_SECONDS}s"
    return 0
  fi

  if awk -v lpc="$load_per_core" -v max="$MAX_LOAD_PER_CORE" 'BEGIN {exit !(lpc > max)}'; then
    log_line "Resource guard: high load/core ${load_per_core} (> ${MAX_LOAD_PER_CORE}), backing off ${RESOURCE_BACKOFF_SECONDS}s"
    return 0
  fi

  return 1
}

get_process_tree_pids() {
  local root="$1"
  local queue="$root"
  local all="$root"
  local current children child

  while [[ -n "$queue" ]]; do
    current="${queue%% *}"
    if [[ "$queue" == *" "* ]]; then
      queue="${queue#* }"
    else
      queue=""
    fi

    children=$(pgrep -P "$current" 2>/dev/null || true)
    for child in $children; do
      all="$all $child"
      if [[ -n "$queue" ]]; then
        queue="$queue $child"
      else
        queue="$child"
      fi
    done
  done

  echo "$all"
}

get_tree_stats() {
  local root="$1"
  local pids pid rss cpu
  local total_rss=0
  local total_cpu="0.0"

  pids=$(get_process_tree_pids "$root")
  for pid in $pids; do
    rss=$(ps -o rss= -p "$pid" 2>/dev/null | awk 'NR==1 {print int($1)}')
    cpu=$(ps -o %cpu= -p "$pid" 2>/dev/null | awk 'NR==1 {print $1}')

    [[ -n "$rss" ]] && total_rss=$((total_rss + rss))
    [[ -n "$cpu" ]] && total_cpu=$(awk -v a="$total_cpu" -v b="$cpu" 'BEGIN {printf "%.1f", a + b}')
  done

  echo "$total_rss $total_cpu"
}

kill_process_tree() {
  local root="$1"
  local pids pid

  pids=$(get_process_tree_pids "$root")
  for pid in $pids; do
    kill -TERM "$pid" 2>/dev/null || true
  done

  sleep "$CODEX_KILL_AFTER_SECONDS"

  for pid in $pids; do
    if kill -0 "$pid" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
  done
}

run_codex_attempt() {
  local out_file="$1"
  local start_ts now elapsed pid high_cpu_hits rss_kb cpu_pct rss_mb

  high_cpu_hits=0
  start_ts=$(date +%s)

  {
    echo "===== attempt start $(date -u +%Y-%m-%dT%H:%M:%SZ) ====="
    echo "limits: timeout=${CODEX_TIMEOUT_SECONDS}s vmem=${CODEX_MAX_VMEM_KB}KB rss=${CODEX_MAX_RSS_MB}MB cpu=${CODEX_MAX_CPU_PERCENT}% nice=${CODEX_NICE_LEVEL}"
  } >> "$out_file"

  (
    if [[ "$CODEX_MAX_VMEM_KB" -gt 0 ]]; then
      ulimit -Sv "$CODEX_MAX_VMEM_KB" 2>/dev/null || true
    fi

    if [[ "$CODEX_NICE_LEVEL" -gt 0 ]] && command -v nice >/dev/null 2>&1; then
      exec nice -n "$CODEX_NICE_LEVEL" codex "${CODEX_ARGS[@]}"
    else
      exec codex "${CODEX_ARGS[@]}"
    fi
  ) >> "$out_file" 2>&1 &

  pid=$!

  while kill -0 "$pid" 2>/dev/null; do
    sleep "$WATCHDOG_INTERVAL_SECONDS"

    now=$(date +%s)
    elapsed=$((now - start_ts))

    read -r rss_kb cpu_pct <<EOF
$(get_tree_stats "$pid")
EOF

    [[ -z "$rss_kb" ]] && rss_kb=0
    [[ -z "$cpu_pct" ]] && cpu_pct=0
    rss_mb=$((rss_kb / 1024))

    echo "[watchdog] elapsed=${elapsed}s pid=${pid} rss=${rss_mb}MB cpu=${cpu_pct}%" >> "$out_file"

    if [[ "$CODEX_MAX_RSS_MB" -gt 0 ]] && [[ "$rss_mb" -gt "$CODEX_MAX_RSS_MB" ]]; then
      echo "[watchdog] RSS limit exceeded (${rss_mb}MB > ${CODEX_MAX_RSS_MB}MB)" >> "$out_file"
      kill_process_tree "$pid"
      wait "$pid" 2>/dev/null || true
      return 137
    fi

    if [[ "$CODEX_MAX_CPU_PERCENT" -gt 0 ]]; then
      if awk -v c="$cpu_pct" -v m="$CODEX_MAX_CPU_PERCENT" 'BEGIN {exit !(c > m)}'; then
        high_cpu_hits=$((high_cpu_hits + 1))
      else
        high_cpu_hits=0
      fi

      if [[ "$high_cpu_hits" -ge "$CODEX_MAX_CPU_HITS" ]]; then
        echo "[watchdog] CPU limit exceeded for ${high_cpu_hits} checks (${cpu_pct}% > ${CODEX_MAX_CPU_PERCENT}%)" >> "$out_file"
        kill_process_tree "$pid"
        wait "$pid" 2>/dev/null || true
        return 143
      fi
    fi

    if [[ "$elapsed" -ge "$CODEX_TIMEOUT_SECONDS" ]]; then
      echo "[watchdog] Timeout reached (${CODEX_TIMEOUT_SECONDS}s)" >> "$out_file"
      kill_process_tree "$pid"
      wait "$pid" 2>/dev/null || true
      return 124
    fi
  done

  wait "$pid"
}

# --- Main loop ---
ITERATION=0
while true; do
  ITERATION=$((ITERATION + 1))
  TS=$(date +%Y%m%d-%H%M%S)
  echo "$TS iteration=$ITERATION" > "$HEARTBEAT_FILE"

  CODEX_RUNNING=$(count_codex_processes)
  if [[ "$CODEX_RUNNING" -ge "$MAX_CONCURRENT_CODEX_PROCS" ]]; then
    log_line "Resource guard: ${CODEX_RUNNING} global codex processes running (limit ${MAX_CONCURRENT_CODEX_PROCS}), backing off ${RESOURCE_BACKOFF_SECONDS}s"
    sleep "$RESOURCE_BACKOFF_SECONDS"
    continue
  fi

  REPO_CODEX_RUNNING=$(count_repo_codex_processes)
  if [[ "$REPO_CODEX_RUNNING" -ge "$MAX_CONCURRENT_REPO_CODEX_PROCS" ]]; then
    log_line "Resource guard: ${REPO_CODEX_RUNNING} codex process already targeting ${REPO_DIR} (limit ${MAX_CONCURRENT_REPO_CODEX_PROCS}), backing off ${RESOURCE_BACKOFF_SECONDS}s"
    sleep "$RESOURCE_BACKOFF_SECONDS"
    continue
  fi

  if should_backoff_for_system_load; then
    sleep "$RESOURCE_BACKOFF_SECONDS"
    continue
  fi

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
Iteration $ITERATION at $TS. Fix the highest-priority issue you can identify from the code and logs.

## Unattended Loop Flags
ALLOW_PI_DEPLOY=$ALLOW_PI_DEPLOY

$SAFETY_OVERRIDE"

  # Pick endpoint
  if pick_endpoint; then
    CODEX_ARGS=(
      -a never -s workspace-write exec --json
      --model "$LB_MODEL"
      -c "model_provider=azure"
      -c "model_providers.azure.name=azure"
      -c "model_providers.azure.env_key=AZURE_OPENAI_API_KEY"
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

  : > "$LOG_DIR/$TS.out.log"

  RETRY=0
  while [[ $RETRY -lt $CODEX_MAX_RETRIES ]]; do
    if run_codex_attempt "$LOG_DIR/$TS.out.log"; then
      echo "[$TS] Iteration $ITERATION completed successfully."
      break
    else
      EXIT_CODE=$?
      RETRY=$((RETRY + 1))
      echo "[$TS] Codex exited $EXIT_CODE (retry $RETRY/$CODEX_MAX_RETRIES)"

      if [[ "$EXIT_CODE" -eq 137 || "$EXIT_CODE" -eq 143 ]]; then
        echo "[$TS] Resource watchdog tripped; cooling down ${RESOURCE_BACKOFF_SECONDS}s"
        sleep "$RESOURCE_BACKOFF_SECONDS"
      else
        sleep "$CODEX_RETRY_DELAY_SECONDS"
      fi
    fi
  done

  # Push any changes (ignore transient lockfile noise)
  git checkout -- automation/.codex-loop.lock/pid 2>/dev/null || true

  if [[ -n "$(git status --porcelain)" ]]; then
    git add -A
    git reset automation/.codex-loop.lock/pid 2>/dev/null || true

    if [[ -n "$(git diff --cached --name-only)" ]]; then
      git commit -m "codex: pi-media-player iteration $ITERATION auto-fix ($TS)" --no-verify 2>/dev/null || true
      git push origin "$BRANCH_NAME" 2>/dev/null || true
      echo "[$TS] Pushed changes from iteration $ITERATION"
    else
      echo "[$TS] No committable changes (transient files only)."
    fi
  fi

  echo "[$TS] Sleeping ${SLEEP_SECONDS}s..."
  sleep "$SLEEP_SECONDS"
done
