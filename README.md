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
| `serve` | Serve the web view over TLS on loopback; prints a single-use login URL |
| `web ca\|login\|client-cert` | Show the CA to trust, get a fresh login URL from the running server, write a client certificate for `serve --mtls` |
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

`restore` and `respawn` put each session back in the tmux session and window it
was last seen in (recorded by the hook and by every refresh; shown by `show`).
A session with no recorded placement opens in the `agents` session. Setting
`CLAUDE_MONITOR_TMUX_SESSION` or `tmuxSession` in `config.json` forces every
session into that one tmux session instead; window names are still kept.

`claude-monitor --help` and `claude-monitor <command> --help` list every flag.
Nothing here sends session content off the machine: `summarize` talks only to
a loopback endpoint and `links check` only runs the `cortexiphan` client when
it is on `PATH`; without it the link is reported unreachable and nothing else
changes.

## Web view

`claude-monitor serve [--port N] [--bind ADDR] [--mtls]` serves the web view on
loopback only (`127.0.0.1` or `::1`; any other `--bind` is refused). `--port 0`
picks a free port. It speaks only HTTPS (TLS 1.3); there is no plain-HTTP
listener. `serve` stops cleanly on SIGTERM.

**Trust the certificate once.** On first run `serve` makes a certificate
authority for this install in `<state-dir>/tls/` (directory 0700, files 0600)
and a leaf certificate for `127.0.0.1`, `::1` and `localhost`. The authority is
name-constrained to those names, so trusting it cannot vouch for any other
site. `claude-monitor web ca` prints its path and the import hint;
`web ca --print` writes the certificate as PEM. Import it as an authority in
Firefox (Settings > Privacy & Security > Certificates > View Certificates >
Authorities > Import) or Chromium (Settings > Privacy and security > Security >
Manage certificates > Authorities > Import). The leaf is renewed on its own
when under 14 days remain; the authority lasts five years.

**Log in with a single-use URL.** The first line of stdout is a login URL:

```text
https://127.0.0.1:41234/login?code=...
```

The code is 256 random bits, valid for five minutes and one use, and exists
only in the server's memory. Opening it trades it for a session cookie
(`__Host-` prefixed, `Secure`, `HttpOnly`, `SameSite=Strict`) holding a random
id, and redirects to `/`. Sessions live in memory: they end after 8 hours idle
or 24 hours in total, and when `serve` restarts. `claude-monitor web login`
asks the running server, over an owner-only unix socket
(`<state-dir>/web.sock`), for a fresh URL. Five failed logins or
unauthenticated requests in a minute make the server answer 429 with
`Retry-After` until the window passes; a signed-in browser is not affected.

**Optional client certificate.** `serve --mtls` additionally requires a client
certificate from the same authority in the TLS handshake; a connection without
one is refused before any HTTP is read. `claude-monitor web client-cert
[--out FILE] [--password-file FILE]` writes a PKCS#12 bundle (default
`<state-dir>/tls/client.p12`, mode 0600) to import in the browser under "Your
Certificates"; without `--password-file` it generates a password and prints it
once. Login still applies on top of it.

**What this protects against, and what it does not.** It stops a process or a
web page that can merely reach the port: it cannot log in without a code, cannot
forge a session, and what crosses loopback is encrypted. Cross-site requests are blocked by the `Host` allow-list,
`SameSite=Strict`, a per-session CSRF header and a restrictive
content-security-policy. It does not stop anything running as your user: such a
process can read the CA key and state directory, run `claude-monitor web login`
itself, or read the printed URL from the terminal or the journal while the
code is still valid. Root can do anything. The state directory is the trust
boundary; see `docs/web-design.md`.

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
