#!/usr/bin/env bash
# Runs `claude-monitor list --json` against the REAL $HOME and prints how many
# sessions it found per agent. Read-only on ~/.claude and ~/.copilot: the
# store and the runtime directory are throwaway temp dirs, and claude-monitor
# only reads the agents' own directories.
#
# Usage: scripts/smoke-real.sh [path/to/claude-monitor]
#   Default binary: ${CARGO_TARGET_DIR:-target}/debug/claude-monitor
set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
  exec nix shell nixpkgs#jq --command "$0" "$@"
fi

bin=${1:-${CLAUDE_MONITOR_BIN:-${CARGO_TARGET_DIR:-target}/debug/claude-monitor}}
if [ ! -x "$bin" ]; then
  echo "smoke-real: $bin not found; run cargo build or pass the binary path" >&2
  exit 2
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

out=$(
  XDG_RUNTIME_DIR="$scratch/run" "$bin" --json --state-dir "$scratch/state" \
    list --include-dismissed --expand-finished
)

echo "claude-monitor list --json ($(date -u +%Y-%m-%dT%H:%M:%SZ), HOME=$HOME)"
echo "total: $(jq '.sessions | length' <<<"$out")"
echo "per agent and state:"
jq -r '
  .sessions
  | group_by(.agent_kind)[]
  | . as $agent
  | "  \($agent[0].agent_kind): \($agent | length)  ("
    + ($agent | group_by(.state) | map("\(.[0].state)=\(length)") | join(", "))
    + ")"
' <<<"$out"
