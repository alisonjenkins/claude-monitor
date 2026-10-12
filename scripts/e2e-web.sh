#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2329
# Browser end-to-end check of the web view (headless Chromium via Playwright).
#
# Seeds a private store through the real `hook` CLI, serves it on a free
# loopback port and drives the page with scripts/e2e-web.py. Isolation: HOME,
# XDG_*, the tmux socket and the launcher stub all live in one mktemp dir; the
# browser is a fresh headless profile. Nothing touches the real ~/.claude,
# ~/.copilot, tmux server or state dir, and no real agent is started.
#
# Usage: scripts/e2e-web.sh [path/to/claude-monitor]   (run `cargo build` first)
# Env:   E2E_OUT      evidence directory (screenshots + assertions.log)
#        E2E_PYTHON   python with the `playwright` package (default python3)
#        E2E_CHROMIUM chromium executable (default: Playwright's own)
# Nix, without a global playwright (a dev shell's LD_LIBRARY_PATH breaks the
# browser, so clear it):
#   env -u LD_LIBRARY_PATH nix-shell -p 'python3.withPackages (ps: [ps.playwright])' \
#     --run 'E2E_PYTHON=python E2E_CHROMIUM=$(command -v chromium) scripts/e2e-web.sh'
set -euo pipefail
[ -z "${E2E_TRACE:-}" ] || set -x

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${1:-${CLAUDE_MONITOR_BIN:-${CARGO_TARGET_DIR:-$REPO/target}/debug/claude-monitor}}
[ -x "$BIN" ] || { echo "e2e-web: $BIN not found; run cargo build or pass the binary" >&2; exit 2; }
for tool in tmux jq curl; do
  command -v "$tool" >/dev/null || { echo "e2e-web: $tool is required" >&2; exit 2; }
done

TMP=$(mktemp -d)
SOCK=$TMP/sock
SERVE_PID=""
cleanup() {
  if [ -n "$SERVE_PID" ]; then
    kill "$SERVE_PID" 2>/dev/null || true
    wait "$SERVE_PID" 2>/dev/null || true
  fi
  pkill -f "$TMP/holder" 2>/dev/null || true
  tmux -S "$SOCK" kill-server 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

export HOME=$TMP/home
export XDG_CONFIG_HOME=$HOME/.config
export XDG_STATE_HOME=$TMP/state
export XDG_DATA_HOME=$TMP/data
export XDG_RUNTIME_DIR=$TMP/run
export CLAUDE_MONITOR_TMUX_SOCKET=$SOCK
unset TMUX TMUX_PANE CLAUDE_CONFIG_DIR COPILOT_HOME CLAUDE_MONITOR_TMUX_SESSION
export CLAUDE_MONITOR_LAUNCHER_CLAUDE="$TMP/stub.sh {session_id}"
export E2E_BIN=$BIN E2E_TMP=$TMP E2E_SOCK=$SOCK E2E_REPO=$REPO
export E2E_OUT=${E2E_OUT:-$TMP/evidence}
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" "$E2E_OUT"
chmod 700 "$XDG_RUNTIME_DIR"

# Stub agent for respawn: records its cwd, then idles on the private tmux.
cat >"$TMP/stub.sh" <<'EOF'
#!/bin/sh
echo "$1 $(pwd -P)" >>"$E2E_TMP/cwd.log"
sleep 120
EOF
chmod +x "$TMP/stub.sh"

FIX=$REPO/tests/fixtures/claude
sid() { printf '%08d-1111-4111-8111-111111111111' "$1"; }

# payload <n> <fixture>: the fixture re-pointed at seed n's directory.
payload() {
  local n=$1 fixture=$2 id dir encoded
  id=$(sid "$n")
  dir=$TMP/work-$n
  mkdir -p "$dir"
  encoded=$(printf '%s' "$dir" | sed 's#[/.]#-#g')
  mkdir -p "$HOME/.claude/projects/$encoded"
  : >"$HOME/.claude/projects/$encoded/$id.jsonl"
  jq -c --arg id "$id" --arg cwd "$dir" \
    --arg tp "$HOME/.claude/projects/$encoded/$id.jsonl" \
    '.session_id=$id | .cwd=$cwd | .transcript_path=$tp' \
    "$FIX/$fixture.json" >"$TMP/p-$n-$fixture.json"
}

# Hook parent that stays alive, so the session is not marked interrupted.
# The hook runs under a bash named holder-<n> whose pid is the recorded agent.
cp "$(command -v bash)" "$TMP/holder" 2>/dev/null || cp /bin/sh "$TMP/holder"
hook_live() { # hook_live <n> <fixture>
  payload "$1" "$2"
  setsid "$TMP/holder" -c '"$1" hook <"$2"; sleep 600' _ "$BIN" "$TMP/p-$1-$2.json" >/dev/null 2>&1 &
  sleep 0.6
}
hook_dead() { # hook_dead <n> <fixture>: parent exits, so the pid is dead
  payload "$1" "$2"
  bash -c '"$1" hook <"$2"' _ "$BIN" "$TMP/p-$1-$2.json"
}
mon() { "$BIN" --json "$@"; }

# 1 working, 2 needs_permission (alpha), 3 interrupted, 4 finished (no
# project), 5 unseen (transcript only), 6 finished (alpha).
hook_live 1 SessionStart-startup
hook_live 1 UserPromptSubmit
hook_live 2 SessionStart-startup
hook_live 2 Notification-permission_prompt
hook_dead 3 SessionStart-startup
hook_dead 4 SessionStart-startup
hook_dead 4 SessionEnd-prompt_input_exit
hook_dead 6 SessionStart-startup
hook_dead 6 SessionEnd-prompt_input_exit
mkdir -p "$HOME/.claude/projects/-unseen-five"
mkdir -p "$TMP/work-5"
printf '{"type":"user","sessionId":"%s","timestamp":"2026-10-12T09:00:00Z","cwd":"%s","message":{"content":"old discovered work"}}\n' \
  "$(sid 5)" "$TMP/work-5" >"$HOME/.claude/projects/-unseen-five/$(sid 5).jsonl"

internal_id() { # internal_id <n>: store id of seed n
  mon list --include-dismissed --expand-finished |
    jq -r --arg id "$(sid "$1")" '.sessions[] | select(.agent_session_id == $id) | .id'
}
mon group alpha "$(internal_id 2)" "$(internal_id 6)" >/dev/null
printf -- '- [x] T001 first\n- [ ] T002 second\n- [ ] T003 third\n' >"$TMP/work-2/todo.md"
mon progress bind "$(internal_id 2)" "$TMP/work-2/todo.md" >/dev/null

# Prepared for the live-update check: session 1 starts waiting on permission.
payload 1 Notification-permission_prompt

mon list --include-dismissed --expand-finished >"$E2E_OUT/seed-list.json"

# Not through `mon`: backgrounding a function forks a subshell, `$!` is then
# the subshell and `kill` leaves the server running.
"$BIN" --json serve --port 0 >"$TMP/serve.out" 2>"$TMP/serve.err" &
SERVE_PID=$!
for _ in $(seq 1 50); do
  grep -q 'token=' "$TMP/serve.out" 2>/dev/null && break
  sleep 0.1
done
URL=$(grep -o 'http://[^ "]*token=[^ "]*' "$TMP/serve.out" | head -1)
[ -n "$URL" ] || { echo "e2e-web: serve printed no URL" >&2; cat "$TMP/serve.out" "$TMP/serve.err" >&2; exit 1; }
export E2E_URL=$URL

PY=${E2E_PYTHON:-python3}
rc=0
"$PY" "$REPO/scripts/e2e-web.py" || rc=$?
echo "evidence: $E2E_OUT"
exit "$rc"
