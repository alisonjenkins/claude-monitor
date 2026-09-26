# claude-monitor

A terminal dashboard showing the live state of every Claude Code session
running in tmux, so you can see which ones need you and jump to them.

## How it works

Claude Code hooks run `claude-monitor hook` on each event; it reads the hook
JSON on stdin and writes one small JSON status file per session to
`$XDG_RUNTIME_DIR/claude-monitor/` (fallback `/tmp/claude-monitor-<uid>`).
The dashboard watches that directory and redraws live.

| Hook event | Session state |
|---|---|
| SessionStart | Idle (your turn) |
| UserPromptSubmit, PreToolUse, PostToolUse | Working |
| PermissionRequest, Notification (`permission_prompt`) | Needs permission |
| Notification (`idle_prompt`), Stop | Idle (your turn) |
| SessionEnd | Removed |

## Install (home-manager)

```nix
# in your flake
inputs.claude-monitor.url = "github:alisonjenkins/claude-monitor";
```

```nix
# in your home-manager module
imports = [ inputs.claude-monitor.homeManagerModules.default ];

programs.claude-monitor.enable = true;
```

The module installs the binary and registers the hooks in Claude Code's
settings with the absolute store path.

## Usage

Run `claude-monitor` inside tmux.

| Key | Action |
|---|---|
| `j` / `Down` | Move down |
| `k` / `Up` | Move up |
| `Enter` | Jump to the session's tmux pane |
| `d` | Hide the entry until its state next changes |
| `q` / `Esc` | Quit |

Sessions needing permission sort first, then idle, then working; a desktop
notification fires when a session starts needing you.

## Development

```bash
nix develop
cargo test
cargo clippy --all-targets -- -D warnings
nix flake check
nix build
```

