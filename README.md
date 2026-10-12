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

## Commands

`claude-monitor` with no subcommand opens the dashboard. Every command takes
`--json` (machine-readable output; errors print as `{"ok":false,"error":"..."}`),
`--state-dir <PATH>` and `--no-refresh`. Exit codes: 0 success, 1 error,
2 refused, 3 partial failure.

| Command | What it does |
|---|---|
| `list` | List sessions grouped by view section |
| `show <session>` | Show one session in full |
| `hook` | Update session state from an agent hook event (reads hook JSON on stdin) |
| `reconcile` | Mark sessions whose process, tmux pane or boot is gone as interrupted |
| `watch` | Log one line per session state change until SIGINT or SIGTERM |
| `restore` | Start interrupted sessions again in tmux; without `--yes`, only print the plan |
| `respawn <session>...` | Start named sessions again in tmux, resuming their conversations |
| `boot-check` | Count sessions a reboot or crash interrupted, notify once, start nothing |
| `summarize <target>` | Ask a model on this machine for a short summary; never automatic |
| `serve` | Serve the web view on loopback; prints the URL that opens it |
| `progress [session]` | Show a session's plan progress and the file it comes from |
| `progress bind <owner> [file]` | Choose the plan a session's progress comes from |
| `project add\|rename\|set-status` | Create, rename or set the status of a project |
| `group <project> <session>...` | Put sessions in a project (`-` removes them from theirs) |
| `note <target> <text>` | Attach a note to a session or project |
| `park` / `unpark` | Flag or unflag a session or project as parked |
| `dismiss` / `undismiss` | Hide or show a session or project; nothing is deleted |
| `done` / `undone` | Move finished sessions to their project's Done section, or back |
| `link <owner> <node-id>` / `unlink` | Record or remove a link to a node in the external graph |
| `links check` | Show linked records whose two sides disagree; writes neither |
| `export cortexiphan` | Write projects, steps, notes and sessions as JSON Lines for cortexiphan |

`claude-monitor --help` and `claude-monitor <command> --help` list every flag.
Nothing here sends session content off the machine: `summarize` talks only to
a loopback endpoint and `links check` only runs the `cortexiphan` client when
it is on `PATH`; without it the link is reported unreachable and nothing else
changes.

## Web view

`claude-monitor serve [--port N] [--bind ADDR]` serves the web view on
loopback only (`127.0.0.1` or `::1`; any other `--bind` is refused). `--port 0`
picks a free port. The first line of stdout is the URL, which carries a token:

```text
http://127.0.0.1:41234/?token=...
```

Opening it sets a cookie and redirects; every other request needs that cookie.
The token is kept in `web.token` in the state directory (mode 0600). `serve`
stops cleanly on SIGTERM.

## Adding an agent kind

An agent kind is one module in the library; the store, views, respawn planner,
progress and export do not change. The full contract is
`specs/008-ai-control-centre/contracts/agent-kind.md` in the spec repo.

1. Implement `AgentKind` (`src/agents/mod.rs`) in `src/agents/<name>.rs`:
   `id`, `parse_hook` (pure, payload to events), `discover` (read-only),
   `launch` (resume and fresh) and `resume_preconditions`.
2. Register it in `agents::kinds()`.
3. Put real payloads under `tests/fixtures/<name>/` and test them: payloads to
   events, `launch` in both modes, and a respawn through the private-tmux
   harness (`tests/common`).
4. Run the conformance checks: `tests/third_agent_kind.rs` shows a kind that
   exists only in a test flowing through ingest, `list`, `show` and discovery.
5. Register its hooks in `nix/hm-module.nix`, or rely on `discover` if the
   agent has none. Launch commands are configuration, not code:
   `programs.claude-monitor.launchers.<name>` with the placeholders
   `{session_id}`, `{cwd}` and `{name}`.

Current kinds: `claude` and `copilot`.

## Scripts

Both build on `cargo build` output; pass a binary path, or set
`CLAUDE_MONITOR_BIN`.

- `scripts/e2e-s3-s4.sh`: end-to-end run of the respawn and boot-flow
  scenarios with a stub agent on a private tmux server. Needs tmux, jq,
  dbus-daemon, dbus-monitor and script; touches nothing outside a temp dir.
- `scripts/smoke-real.sh`: runs `list --json` against the real `$HOME` and
  prints session counts per agent. Read-only on `~/.claude` and `~/.copilot`;
  the store is a throwaway temp dir.

## Development

```bash
nix develop
cargo test
cargo clippy --all-targets -- -D warnings
nix flake check
nix build
```


## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
