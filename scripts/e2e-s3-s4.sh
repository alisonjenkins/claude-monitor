#!/usr/bin/env bash
# shellcheck disable=SC2016
# End-to-end run of quickstart scenarios S3 (respawn) and S4 (boot flow)
# against the built binary, with a stub agent on a PRIVATE tmux server.
#
# Isolation: HOME, XDG_*, the tmux socket (CLAUDE_MONITOR_TMUX_SOCKET), the
# launcher template and the D-Bus session bus (for the notification) all point
# into one mktemp dir. Nothing touches the real tmux server, ~/.claude,
# ~/.copilot or the real state dir, and no real agent is started.
#
# Usage: scripts/e2e-s3-s4.sh [path/to/claude-monitor]   (run `cargo build` first)
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${1:-${CLAUDE_MONITOR_BIN:-${CARGO_TARGET_DIR:-$REPO/target}/debug/claude-monitor}}
[ -x "$BIN" ] || { echo "e2e: $BIN not found; run cargo build or pass the binary" >&2; exit 2; }
for tool in tmux jq dbus-daemon dbus-monitor script; do
  command -v "$tool" >/dev/null || { echo "e2e: $tool is required" >&2; exit 2; }
done

TMP=$(mktemp -d)
SOCK=$TMP/sock
DBUS_PID=""
MON_PID=""
ATTACH_PID=""
pass=0

cleanup() {
  [ -n "$ATTACH_PID" ] && kill "$ATTACH_PID" 2>/dev/null || true
  [ -n "$MON_PID" ] && kill "$MON_PID" 2>/dev/null || true
  [ -n "$DBUS_PID" ] && kill "$DBUS_PID" 2>/dev/null || true
  tmux -S "$SOCK" kill-server 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

ok() { pass=$((pass + 1)); echo "  ok   $*"; }
die() { echo "  FAIL $*" >&2; exit 1; }
check() { # check <description> <command...>
  local what=$1
  shift
  if "$@" >/dev/null; then ok "$what"; else die "$what"; fi
}

export HOME=$TMP/home
export XDG_CONFIG_HOME=$HOME/.config
export XDG_STATE_HOME=$TMP/state
export XDG_DATA_HOME=$TMP/data
export XDG_RUNTIME_DIR=$TMP/run
export CLAUDE_MONITOR_TMUX_SOCKET=$SOCK
unset TMUX TMUX_PANE CLAUDE_CONFIG_DIR COPILOT_HOME CLAUDE_MONITOR_TMUX_SESSION
export CLAUDE_MONITOR_LAUNCHER_CLAUDE="$TMP/stub.sh {session_id}"
export E2E_BIN=$BIN E2E_TMP=$TMP
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# The stub agent: records its cwd and args, announces itself with a resume
# SessionStart if the test prepared one (its parent pid is this live shell),
# then idles.
cat >"$TMP/stub.sh" <<'EOF'
#!/bin/sh
echo "$1 $(pwd -P)" >>"$E2E_TMP/cwd.log"
echo "$*" >>"$E2E_TMP/args.log"
if [ -f "$E2E_TMP/resume-$1.json" ]; then
  "$E2E_BIN" hook <"$E2E_TMP/resume-$1.json"
fi
sleep 60
EOF
chmod +x "$TMP/stub.sh"

mon() { "$BIN" --json "$@"; }

# New scenario: empty store, no tmux server, empty stub logs.
reset_world() {
  tmux -S "$SOCK" kill-server 2>/dev/null || true
  rm -rf "$XDG_STATE_HOME" "$HOME/.claude" "$TMP"/work-* "$TMP"/resume-*.json "$TMP"/*.log
  mkdir -p "$XDG_STATE_HOME" "$HOME/.claude"
}

# seed_session <n>: ingest a SessionStart through the real hook with a parent
# that exits straight after, so the recorded pid is dead and the next refresh
# marks the session interrupted. Also makes the transcript resume needs.
sid() { printf '%08d-1111-4111-8111-111111111111' "$1"; }
seed_session() {
  local n=$1 id dir encoded
  id=$(sid "$n")
  dir=$TMP/work-$n
  mkdir -p "$dir"
  encoded=$(printf '%s' "$dir" | sed 's#[/.]#-#g')
  mkdir -p "$HOME/.claude/projects/$encoded"
  : >"$HOME/.claude/projects/$encoded/$id.jsonl"
  jq -c --arg id "$id" --arg cwd "$dir" --arg tp "$HOME/.claude/projects/$encoded/$id.jsonl" \
    '.session_id=$id | .cwd=$cwd | .transcript_path=$tp' \
    "$REPO/tests/fixtures/claude/SessionStart-startup.json" >"$TMP/start-$n.json"
  jq -c --arg id "$id" --arg cwd "$dir" --arg tp "$HOME/.claude/projects/$encoded/$id.jsonl" \
    '.session_id=$id | .cwd=$cwd | .transcript_path=$tp' \
    "$REPO/tests/fixtures/claude/SessionStart-resume.json" >"$TMP/resume-pending-$n.json"
  bash -c '"$1" hook <"$2"' _ "$BIN" "$TMP/start-$n.json"
}

state_of() { # state_of <n>: state of the logical session for seed n
  mon list --include-dismissed --expand-finished |
    jq -r --arg id "$(sid "$1")" '[.sessions[] | select(.agent_session_id == $id)] | map(.state) | join(",")'
}
internal_id() { # internal_id <n>: store id of seed n
  mon list --include-dismissed --expand-finished |
    jq -r --arg id "$(sid "$1")" ".sessions[] | select(.agent_session_id == \$id) | .id"
}
rows_of() { # rows_of <n>: number of rows for seed n
  mon list --include-dismissed --expand-finished |
    jq -r --arg id "$(sid "$1")" '[.sessions[] | select(.agent_session_id == $id)] | length'
}
wait_for_file() { # wait_for_file <path> [lines]
  for _ in $(seq 1 50); do
    [ -f "$1" ] && [ "$(wc -l <"$1")" -ge "${2:-1}" ] && return 0
    sleep 0.1
  done
  return 1
}
tmux_p() { tmux -S "$SOCK" "$@"; }
no_server() { ! tmux_p list-sessions >/dev/null 2>&1; }
run_rc() { # run_rc <outfile> <cmd...>: store stdout, echo the exit code
  local out=$1 rc=0
  shift
  "$@" >"$out" 2>"$out.err" || rc=$?
  echo "$rc"
}

echo "S3: respawn"

echo " dry run"
reset_world
seed_session 1
rc=$(run_rc "$TMP/out" mon restore)
check "restore without --yes exits 0" test "$rc" = 0
check "plan says executed=false and lists the session" \
  jq -e --arg cwd "$TMP/work-1" '.executed == false and (.would_start | map(.cwd) == [$cwd])' "$TMP/out"
check "no tmux server was started" no_server
check "stub never ran" test ! -e "$TMP/cwd.log"

echo " single respawn lands in the recorded directory, then the resume SessionStart links"
reset_world
seed_session 1
mv "$TMP/resume-pending-1.json" "$TMP/resume-$(sid 1).json"
rc=$(run_rc "$TMP/out" mon restore --all-interrupted --yes)
check "restore --all-interrupted --yes exits 0" test "$rc" = 0
check "stub started" wait_for_file "$TMP/cwd.log"
check "tmux session agents exists on the private socket" tmux_p has-session -t =agents
check "stub cwd is the recorded cwd" grep -qxF "$(sid 1) $TMP/work-1" "$TMP/cwd.log"
check "stub args contain the session id" grep -qF "$(sid 1)" "$TMP/args.log"
check "a window pane is in the recorded cwd" \
  bash -c 'tmux -S "$1" list-panes -a -F "#{pane_current_path}" | grep -qxF "$2"' _ "$SOCK" "$TMP/work-1"
sleep 1
check "overview shows ONE row for the session" test "$(rows_of 1)" = 1
check "that row is revived in place: idle (data-model: SessionStart resume -> idle)" test "$(state_of 1)" = idle

echo " five at once"
reset_world
for n in 1 2 3 4 5; do seed_session "$n"; done
rc=$(run_rc "$TMP/out" mon restore --all-interrupted --yes)
check "restore of five exits 0" test "$rc" = 0
check "five stubs started" wait_for_file "$TMP/cwd.log" 5
for n in 1 2 3 4 5; do
  check "session $n started in work-$n" grep -qxF "$(sid "$n") $TMP/work-$n" "$TMP/cwd.log"
done

echo " missing directory is refused"
reset_world
seed_session 1
rmdir "$TMP/work-1"
rc=$(run_rc "$TMP/out" mon restore --all-interrupted --yes)
check "exit code 2" test "$rc" = 2
check "message names the missing path" grep -qF "$TMP/work-1" "$TMP/out"
check "no tmux server was started" no_server
check "stub never ran" test ! -e "$TMP/cwd.log"

echo " --skip leaves one interrupted"
reset_world
seed_session 1
seed_session 2
rc=$(run_rc "$TMP/out" mon restore --all-interrupted --yes --skip "$(internal_id 2)")
check "restore --skip exits 0" test "$rc" = 0
check "one stub started" wait_for_file "$TMP/cwd.log"
sleep 0.5
check "only one stub ran" test "$(wc -l <"$TMP/cwd.log")" = 1
check "the skipped session stays interrupted" test "$(state_of 2)" = interrupted

echo "S4: boot flow"
reset_world
seed_session 1
seed_session 2
DBUS_OUT=$(dbus-daemon --session --fork --print-address=1 --print-pid=1)
export DBUS_SESSION_BUS_ADDRESS
DBUS_SESSION_BUS_ADDRESS=$(printf '%s\n' "$DBUS_OUT" | sed -n 1p)
DBUS_PID=$(printf '%s\n' "$DBUS_OUT" | sed -n 2p)
dbus-monitor --address "$DBUS_SESSION_BUS_ADDRESS" \
  "interface='org.freedesktop.Notifications',member='Notify'" >"$TMP/dbus.log" 2>&1 &
MON_PID=$!
sleep 0.5

rc=$(run_rc "$TMP/out" mon boot-check)
check "boot-check exits 0" test "$rc" = 0
check "boot-check reports 2 interrupted" jq -e '.interrupted == 2' "$TMP/out"
sleep 0.5
check "exactly one Notify call on the private bus" test "$(grep -c 'member=Notify' "$TMP/dbus.log")" = 1
check "the notification mentions the count" grep -q '2 AI sessions interrupted' "$TMP/dbus.log"
MARKER=$XDG_STATE_HOME/ai-control-centre/boot-check-pending
check "marker written" test -s "$MARKER"
check "no tmux server or window started" no_server
check "stub never ran" test ! -e "$TMP/cwd.log"

echo " banner through an attached client"
tmux_p -f /dev/null new-session -d -s agents -x 200 -y 50
(sleep 60 | script -qf -c "tmux -S '$SOCK' attach -t agents" "$TMP/client.typescript") >/dev/null 2>&1 &
ATTACH_PID=$!
for _ in $(seq 1 50); do
  [ -n "$(tmux_p list-clients 2>/dev/null)" ] && break
  sleep 0.1
done
check "a client is attached to the private server" test -n "$(tmux_p list-clients)"
out=$("$BIN" boot-check --banner)
check "banner call prints nothing on stdout" test -z "$out"
for _ in $(seq 1 30); do
  grep -q 'interrupted, prefix+A to review' "$TMP/client.typescript" && break
  sleep 0.1
done
check "client received the one-line message" grep -q '2 AI sessions interrupted, prefix+A to review' "$TMP/client.typescript"
check "marker cleared" test ! -e "$MARKER"
before=$(grep -c 'interrupted, prefix+A to review' "$TMP/client.typescript" || true)
out=$("$BIN" boot-check --banner)
sleep 0.5
after=$(grep -c 'interrupted, prefix+A to review' "$TMP/client.typescript" || true)
check "second banner call prints nothing" test -z "$out"
check "second banner call sends the client nothing" test "$before" = "$after"
check "banner JSON says shown=false" bash -c '"$1" --json boot-check --banner | jq -e ".shown == false"' _ "$BIN"

echo "all $pass checks passed"
