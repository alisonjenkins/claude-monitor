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
| `web setup-browser` | Trust the CA and import the client certificate into Firefox, Zen and Chromium profiles for you; idempotent, `--dry-run` first |
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

Every refresh also reads Claude Code's live-session registry
(`~/.claude/sessions/<pid>.json`). A live entry (pid running, start time
matching) revives a session that was marked interrupted, so a
`claude --resume` of the same conversation shows as running again, and it
updates the stored pid, cwd and tmux pane and session name. Finished sessions
stay finished. When the registry lists at least one live session, an active
Claude session that has no live entry, no recorded pid and a state older than
two minutes is marked interrupted (`registry_missing`); a pane id that tmux has
since reused for something else no longer keeps it alive. Copilot sessions are
not in this registry and are never judged by it.

`restore` and `respawn` put each session back in the tmux session and window it
was last seen in (recorded by the hook and by every refresh; shown by `show`).
A session with no recorded placement opens in the `agents` session. Setting
`CLAUDE_MONITOR_TMUX_SESSION` or `tmuxSession` in `config.json` forces every
session into that one tmux session instead; window names are still kept.
Sessions that shared one tmux window as split panes come back as panes of one
window, in pane order, shown as `session:window.pane` in the plan. When every
pane of the original window is being restored the recorded layout is applied;
otherwise (some panes were not Claude sessions, or one was refused) the panes
are tiled. A layout tmux refuses is reported as a note and never fails the
restore. With a session override, only panes recorded in the same tmux session
share a window. The web view shows the same placement before you reboot: on a
session's page, as a hint on its row, and on `/restore` in a read-only panel
that lists the live sessions with where each would be restored, warning when
two would land in the same window and pane.

Sessions you closed on purpose can be dismissed from the web view without
opening each one: waiting rows have a Dismiss button, and `/restore` has
Dismiss selected beside Restore selected. Both show a confirmation first (the
second lists the chosen sessions); nothing is deleted and Undismiss brings a
session back.

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

**Set the browser up in one command.** Importing the CA and the client
certificate by hand is tedious, so `claude-monitor web setup-browser` does it
for NSS-based browsers:

```text
claude-monitor web setup-browser [--browser firefox|zen|chromium|all]
    [--profile PATH]... [--auto-select] [--force] [--dry-run]
```

It needs `certutil` and `pk12util` on `PATH` (on Nix: `nix-shell -p
nss.tools`; it exits 2 with that hint if they are missing) and does not need
`serve` to be running. Start with `--dry-run`: it reads your profiles and
prints what would change, and writes nothing, not even the CA.

- **What it touches.** Firefox profiles from `profiles.ini` under
  `~/.mozilla/firefox` and `~/.config/mozilla/firefox`, Zen profiles under
  `~/.zen` and `~/.config/zen`, and the Chromium, Chrome and Brave database
  `~/.pki/nssdb`; `--profile PATH` adds any other NSS directory. In each it
  trusts the CA for web sites (nickname `claude-monitor local CA`) and imports
  the client certificate (nickname `CN=claude-monitor web client`), only when
  the stored copy is missing or its SHA-256 fingerprint differs; a changed
  certificate replaces the old one and its key rather than sitting beside it.
  The client certificate and key are kept in `<state-dir>/tls/client.pem` and
  `client.key`, so a re-run imports nothing new until they are within 30 days
  of expiry or the CA changes. The bundle password is random, lives in a 0600
  file for the length of the run and is never on a command line.
- **What it refuses.** A Firefox or Zen profile that looks in use (a live
  `lock` or a held `.parentlock`) is refused unless you pass `--force`; a
  profile whose database has a primary password is skipped, not prompted for.
  `certutil` runs with stdin closed and a 30-second timeout.
- **Backups, and how to undo.** Before the first write to a profile it copies
  `cert9.db` and `key4.db` to `<profile>/claude-monitor-backup-<UTC stamp>/`
  (for example `claude-monitor-backup-20261013T090507Z`). Nothing is deleted.
  To undo, close the browser and copy those two files back, or remove the
  entries by hand: `certutil -D -d sql:<profile> -n "claude-monitor local CA"`
  and `certutil -F -d sql:<profile> -n "CN=claude-monitor web client"`.
- **Output and exit codes.** One line per profile (`imported`, `already up to
  date`, `skipped: ...`, `refused: in use`); `--json` gives the same as data.
  Exit 0: done or nothing to do. 2: the NSS tools are missing or no profile was
  found. 3: some profile was refused, skipped or failed (the lines are
  printed).
- **`--auto-select` is opt-in, and global.** It writes a block between
  `// claude-monitor begin` and `// claude-monitor end` in each Firefox and Zen
  profile's `user.js` (never `prefs.js`) that sets
  `security.default_personal_cert` to `"Select Automatically"`. That Firefox
  preference is not scoped to claude-monitor: any site that asks for a client
  certificate gets this one without a prompt. Leave it off if you also use
  client certificates elsewhere. Re-running replaces the block in place and
  leaves the rest of the file alone; to undo, delete the block.
- **Chromium cannot be set to auto-select from a profile.** With
  `--auto-select` the command prints, and does not write, the managed policy
  `AutoSelectCertificateForUrls` limited to `https://127.0.0.1:*` and
  `https://localhost:*` and to certificates issued by this install's CA. Put it
  in `/etc/chromium/policies/managed/claude-monitor.json` (Chrome:
  `/etc/opt/chrome/policies/managed/`, Brave: `/etc/brave/policies/managed/`);
  on NixOS set `programs.chromium.extraOpts.AutoSelectCertificateForUrls`.
- **Home Manager.** `programs.claude-monitor.web.browserSetup.enable = true;`
  runs it on every activation (Linux), with `autoSelect` and `browsers`
  options. It never fails the activation: an open profile or a missing tool
  prints one line instead.
- **The limit.** The client key ends up in the browser profile, readable by
  anything running as your user. This makes the setup painless; it does not
  stop malware that runs as you.

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
