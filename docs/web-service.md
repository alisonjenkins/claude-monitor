# Running the web view as a background service

The Home Manager module can keep `claude-monitor serve` running for you on
Linux and macOS. Without it, start `claude-monitor serve` by hand; the web
view itself is described in the README.

## Options

```nix
programs.claude-monitor = {
  enable = true;
  serve = {
    enable = true;
    port = 8800;   # default 8765
    mtls = false;  # default; true adds --mtls
  };
};
```

| Option | Default | Effect |
|---|---|---|
| `serve.enable` | `false` | Install and start the background service |
| `serve.port` | `8765` | Loopback port; passed as `--port` |
| `serve.mtls` | `false` | Pass `--mtls`: also require a client certificate |

The service never passes `--bind`, so it listens on `127.0.0.1` only.

Pick a port that is free on your machine. The default, 8765, collides with
DynamoDB Local on some setups; if `serve` cannot bind, change `serve.port`.

## What each platform installs

Both are built from one argument list in `nix/hm-module.nix`, so they always
run the same command: `<store path>/bin/claude-monitor serve --port <port>`,
plus `--mtls` when `serve.mtls` is on. A flake check evaluates both.

| | Linux | macOS |
|---|---|---|
| Unit | systemd user unit `claude-monitor-serve` | launchd agent `claude-monitor-serve` |
| Starts | at login (`default.target`) | at load (`RunAtLoad`) |
| Restarts | `Restart=on-failure`, 5 s | `KeepAlive` after a failed exit only, `ThrottleInterval` 10 s |
| Log | `journalctl --user -u claude-monitor-serve` | `~/Library/Logs/claude-monitor-serve.log` |

The service's `PATH` names `tmux` and `git` store paths first, so a respawn
from the web view works without a login shell.

## Getting a login URL

`serve` prints a single-use login URL, valid for five minutes, each time it
starts. It lands in the log above (`journalctl --user -u claude-monitor-serve
-n 5` on Linux, `tail ~/Library/Logs/claude-monitor-serve.log` on macOS).
The macOS agent runs with `Umask` 0077, so that log is mode 0600 and no other
local user can read the URL.
Once it is used or expired, ask the running service for a new one:

```
claude-monitor web login
```

## mTLS and the browser

Every install makes its own CA. Trust it once in the browser:
`claude-monitor web ca` prints its path and how to import it
(`web ca --print` writes the PEM).

With `serve.mtls = true` the browser must also present a client certificate
from that CA, and login still applies on top.

- **Linux.** Set `programs.claude-monitor.web.browserSetup.enable = true;`. On
  each activation it runs `claude-monitor web setup-browser`, which trusts the
  CA and imports the client certificate into Firefox, Zen and Chromium
  profiles. Options: `browserSetup.autoSelect`, `browserSetup.browsers`.
- **macOS, Firefox and Zen.** The same option works: the activation sets up
  the profiles under `~/Library/Application Support/Firefox` and
  `~/Library/Application Support/zen` (plus the Linux-style paths, if present).
  `"chromium"` in `browserSetup.browsers` is ignored, and the activation is
  skipped when nothing else is listed. Without Home Manager, run
  `claude-monitor web setup-browser --browser firefox` (or `zen`) with
  `certutil` and `pk12util` on `PATH`.
- **macOS, Chrome and Safari.** Not automated: they use the Keychain, not an
  NSS database. Run `claude-monitor web ca` and import the CA certificate it
  names into Keychain Access as a trusted authority. With `serve.mtls = true`,
  run `claude-monitor web client-cert [--out FILE] [--password-file FILE]` to
  write a PKCS#12 bundle (default `<state-dir>/tls/client.p12`, mode 0600; the
  generated password is printed once) and import that file into the Keychain.

## Threat model

The service listens on loopback only, so other machines cannot reach it. TLS
protects the traffic on loopback and the login code or client certificate
keeps a web page or a process that merely reaches the port from signing in.
None of this stops malware running as your user: it can read the state
directory, the client key and the browser profile. See the README for the
full list.

## Restart and disable

Linux:

```
systemctl --user restart claude-monitor-serve
systemctl --user status claude-monitor-serve
```

macOS (`gui/$(id -u)` is your login domain):

```
launchctl kickstart -k gui/$(id -u)/org.nix-community.home.claude-monitor-serve
```

The label is the one Home Manager gives the agent
(`org.nix-community.home.claude-monitor-serve`); confirm it with
`launchctl list | grep claude-monitor`.

To disable it, set `programs.claude-monitor.serve.enable = false;` and
switch. Home Manager stops and removes the unit or agent.
