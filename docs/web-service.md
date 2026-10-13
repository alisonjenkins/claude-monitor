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
| Restarts | `Restart=on-failure`, 5 s | `KeepAlive`, `ThrottleInterval` 5 s |
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
- **macOS.** Not automated: `web setup-browser` is NSS-only and the module
  runs it on Linux only. By hand, run `claude-monitor web ca` and import the
  CA certificate it names into your browser or keychain as a trusted
  authority. With `serve.mtls = true`, run `claude-monitor web client-cert
  [--out FILE] [--password-file FILE]` to write a PKCS#12 bundle (default
  `<state-dir>/tls/client.p12`, mode 0600; the generated password is printed
  once) and import that file, for example into Keychain Access.

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
