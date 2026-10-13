{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.claude-monitor;

  # PreToolUse/PostToolUse fire per matched tool name, so the settings schema
  # wants an explicit matcher even for "run on every tool"; the other events
  # have no matcher concept and take a single match-all group.
  toolMatcherEvents = [
    "PreToolUse"
    "PostToolUse"
    "PermissionRequest"
  ];
  hookEvents = [
    "SessionStart"
    "UserPromptSubmit"
    "PreToolUse"
    "PostToolUse"
    "PermissionRequest"
    "Notification"
    "Stop"
    "SessionEnd"
  ];

  hookCommand = "${cfg.package}/bin/claude-monitor hook";

  mkHookGroup =
    event:
    {
      hooks = [
        {
          type = "command";
          command = hookCommand;
        }
      ];
    }
    // lib.optionalAttrs (builtins.elem event toolMatcherEvents) {
      matcher = "*";
    };

  # Copilot CLI hook events are camelCase, unlike Claude's PascalCase ones.
  copilotHookEvents = [
    "sessionStart"
    "sessionEnd"
    "userPromptSubmitted"
    "preToolUse"
    "postToolUse"
    "permissionRequest"
    "notification"
    "agentStop"
  ];

  copilotHooksFile = {
    version = 1;
    hooks = lib.genAttrs copilotHookEvents (event: [
      {
        type = "command";
        bash = "${hookCommand} --agent copilot --event ${event}";
        timeoutSec = 10;
      }
    ]);
  };

  # tmux hooks, verified on tmux 3.6a with a private server (research D12/R9):
  # - pane-exited fires when a pane's process exits and the pane closes
  #   (SIGTERM or SIGKILL), and exposes #{hook_pane}. It does not fire for
  #   kill-pane or kill-window.
  # - pane-died fires instead when remain-on-exit keeps the dead pane, with
  #   #{hook_pane} as well.
  # - window-unlinked fires when a window goes (including kill-pane/kill-window
  #   of its last pane) but exposes only #{hook_window}, never the pane id, so
  #   it runs the full reconcile instead of `--pane`.
  # - None fire when the server itself dies; the login boot-check covers that.
  # Hooks run with the server's environment, so the binary is a store path
  # rather than a PATH lookup. `#{hook_pane}` is expanded by tmux before the
  # shell sees it, so no extra quoting is needed for a `%N` id.
  tmuxHooksSnippet =
    let
      bin = "${cfg.package}/bin/claude-monitor";
    in
    ''
      set-hook -g pane-exited 'run-shell -b "${bin} reconcile --pane #{hook_pane}"'
      set-hook -g pane-died 'run-shell -b "${bin} reconcile --pane #{hook_pane}"'
      set-hook -g window-unlinked 'run-shell -b "${bin} reconcile"'
    '';

  # client-attached fires for each attaching client, so the banner shows in
  # the terminal the user opens first after login. `run-shell -b` keeps tmux
  # from waiting on the job; the store path avoids a PATH lookup, and the
  # command has no shell expansions to quote.
  bootCheckBannerSnippet = ''
    set-hook -g client-attached 'run-shell -b "${cfg.package}/bin/claude-monitor boot-check --banner"'
  '';

  allBrowsers = [
    "firefox"
    "zen"
    "chromium"
  ];
  # The command takes one --browser value; `all` is the full default list.
  browserSetupTargets =
    let
      chosen = lib.unique cfg.web.browserSetup.browsers;
    in
    if lib.sort lib.lessThan chosen == lib.sort lib.lessThan allBrowsers then [ "all" ] else chosen;

  # Activation runs under `set -e`, so the exit status is captured rather than
  # propagated: an in-use profile, a primary password or a missing tool must
  # leave a line, never a failed switch. DRY_RUN is Home Manager's own flag.
  browserSetupScript = ''
    claude_monitor_setup_browser() {
      local browser="$1" status=0
      PATH="${lib.makeBinPath [ pkgs.nss.tools ]}:$PATH" \
        ${cfg.package}/bin/claude-monitor web setup-browser --browser "$browser" \
        ${lib.optionalString cfg.web.browserSetup.autoSelect "--auto-select"} \
        ''${DRY_RUN:+--dry-run} >/dev/null 2>&1 || status=$?
      if [ "$status" -eq 0 ]; then
        echo "claude-monitor: browser setup ($browser): ok"
      else
        echo "claude-monitor: browser setup ($browser): exit $status; run 'claude-monitor web setup-browser --browser $browser' to see why"
      fi
    }
    ${lib.concatMapStringsSep "\n" (b: "claude_monitor_setup_browser ${b}") browserSetupTargets}
  '';
in
{
  options.programs.claude-monitor = {
    tmuxHooks = {
      enable = lib.mkEnableOption "tmux hooks that run claude-monitor when a pane exits" // {
        description = ''
          Run `claude-monitor reconcile --pane` when a tmux pane exits, so a
          session in a closed pane is marked interrupted even with no viewer
          open.

          This appends the hooks to `programs.tmux.extraConfig`, so it works
          only when tmux is managed by Home Manager. If you manage `tmux.conf`
          another way, leave this off and include
          `programs.claude-monitor.tmuxHooks.snippet` yourself.
        '';
      };

      snippet = lib.mkOption {
        type = lib.types.str;
        readOnly = true;
        default = tmuxHooksSnippet;
        defaultText = lib.literalMD "the `set-hook` lines, with the store path of `package`";
        description = "The tmux `set-hook` lines, always computed, for tmux configs not managed by Home Manager.";
      };
    };

    bootCheck = {
      enable = lib.mkEnableOption "a login job that runs `claude-monitor boot-check`" // {
        description = ''
          Run `claude-monitor boot-check` once at login (a systemd user unit
          on Linux, a launchd agent on macOS). If a reboot or crash left
          sessions interrupted it sends one desktop notification and leaves a
          marker for the tmux banner. It never starts an agent: restoring
          sessions stays an explicit `claude-monitor restore`.
        '';
      };

      banner = {
        enable = lib.mkEnableOption "a tmux hook that shows the boot-check banner" // {
          description = ''
            Run `claude-monitor boot-check --banner` when a tmux client
            attaches, so the first terminal after login shows one line
            about interrupted sessions. Appends to `programs.tmux.extraConfig`,
            so it works only when tmux is managed by Home Manager; otherwise
            include `programs.claude-monitor.bootCheck.banner.snippet`
            yourself.
          '';
        };

        snippet = lib.mkOption {
          type = lib.types.str;
          readOnly = true;
          default = bootCheckBannerSnippet;
          defaultText = lib.literalMD "the `set-hook` line, with the store path of `package`";
          description = "The tmux `set-hook` line, always computed, for tmux configs not managed by Home Manager.";
        };
      };
    };

    copilot = {
      enable = lib.mkEnableOption "registering claude-monitor as a GitHub Copilot CLI hook handler" // {
        description = ''
          Write a Copilot CLI hooks file so claude-monitor sees Copilot
          sessions as well as Claude Code ones.

          Caveats:

          - Copilot loads hooks only for folders listed in `trustedFolders`
            in `$COPILOT_HOME/config.json` and silently skips them
            otherwise. Sessions in untrusted folders are still discovered
            passively, but their state is unknown.
          - Copilot does not read the user-level `~/.claude/settings.json`
            hooks, so this separate file is required.
          - `sessionStart` fires lazily, on the first prompt, not when the
            session opens.
        '';
      };

      hooksDir = lib.mkOption {
        type = lib.types.str;
        default = ".copilot/hooks";
        example = ".config/copilot/hooks";
        description = ''
          Directory, relative to `$HOME`, that Copilot reads user-level hook
          files from (`$COPILOT_HOME/hooks`). Change it only when
          `COPILOT_HOME` points somewhere other than `~/.copilot`.
        '';
      };
    };

    watch = {
      enable = lib.mkEnableOption "a systemd user service that runs `claude-monitor watch`" // {
        description = ''
          Run `claude-monitor watch` as a systemd user service (Linux only),
          so session states are tracked and logged to the journal, one line
          per state change, even when no dashboard is open. It only reads the
          agents' own directories and never starts an agent.
        '';
      };
    };

    serve = {
      enable = lib.mkEnableOption "a systemd user service that runs `claude-monitor serve`" // {
        description = ''
          Run `claude-monitor serve` as a systemd user service (Linux only):
          the web view of the same sessions, on a loopback address only
          (127.0.0.1), over TLS 1.3 with a per-install CA that you trust once
          (`claude-monitor web ca`). A single-use login URL, valid for five
          minutes, is printed to the journal each time the service starts;
          read it with `journalctl --user -u claude-monitor-serve -n 5`, or
          ask the running service for a new one with `claude-monitor web login`.
        '';
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 8765;
        description = "The loopback port `claude-monitor serve` listens on.";
      };
    };

    web.browserSetup = {
      enable =
        lib.mkEnableOption "trusting the web view's CA and importing its client certificate into browsers at activation"
        // {
          description = ''
            Run `claude-monitor web setup-browser` on every Home Manager
            activation (Linux only): it trusts the install's local CA and
            imports its client certificate into the NSS database of each
            Firefox and Zen profile and of Chromium, Chrome and Brave
            (`~/.pki/nssdb`). It is idempotent, backs up `cert9.db` and
            `key4.db` before the first write to a profile, skips a profile that
            is open or has a primary password, and never fails the activation:
            it prints one line per browser instead. The client key lives in the
            profile, so this does not stop malware running as your user.
          '';
        };

      autoSelect = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Also make Firefox and Zen pick the client certificate without
          asking, by writing a marked block to each profile's `user.js`
          (`security.default_personal_cert = "Select Automatically"`).

          This is a global browser preference, not one scoped to
          claude-monitor: any site that asks for a client certificate gets
          this one without a prompt. Off by default for that reason. For
          Chromium it changes nothing; the managed policy it needs is printed
          by `claude-monitor web setup-browser --auto-select --browser
          chromium` and must be installed by hand.
        '';
      };

      browsers = lib.mkOption {
        type = lib.types.listOf (
          lib.types.enum [
            "firefox"
            "zen"
            "chromium"
          ]
        );
        default = [
          "firefox"
          "zen"
          "chromium"
        ];
        description = "Which browsers' certificate databases to set up.";
      };
    };

    launchers = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = lib.literalExpression ''
        {
          claude = "claude --resume {session_id}";
          copilot = "copilot --resume={session_id}";
        }
      '';
      description = ''
        Command template per agent kind (`claude`, `copilot`) that
        `claude-monitor respawn` and `restore` run to resume a session.
        Placeholders: `{session_id}`, `{cwd}`, `{name}`. A kind with no
        entry uses its built-in command.
      '';
    };

    tmuxSession = lib.mkOption {
      type = lib.types.str;
      default = "agents";
      description = "Name of the tmux session that respawned agents run in.";
    };

    enable = lib.mkEnableOption "claude-monitor, a TUI dashboard for Claude Code sessions in tmux";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "self.packages.<system>.default";
      description = "The claude-monitor package to install and to register the hook command for.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    home.file = lib.mkIf cfg.copilot.enable {
      "${cfg.copilot.hooksDir}/claude-monitor.json".text = builtins.toJSON copilotHooksFile;
    };

    xdg.configFile."claude-monitor/config.json".text = builtins.toJSON {
      tmuxSession = cfg.tmuxSession;
      launchers = cfg.launchers;
    };

    home.activation = lib.mkIf (cfg.web.browserSetup.enable && pkgs.stdenv.hostPlatform.isLinux) {
      claudeMonitorBrowserSetup = lib.hm.dag.entryAfter [ "writeBoundary" ] browserSetupScript;
    };

    programs.tmux.extraConfig = lib.mkMerge [
      (lib.mkIf cfg.tmuxHooks.enable cfg.tmuxHooks.snippet)
      (lib.mkIf cfg.bootCheck.banner.enable cfg.bootCheck.banner.snippet)
    ];

    systemd.user.services.claude-monitor-watch =
      lib.mkIf (cfg.watch.enable && pkgs.stdenv.hostPlatform.isLinux)
        {
          Unit.Description = "claude-monitor: log AI agent session state changes";
          Service = {
            ExecStart = "${cfg.package}/bin/claude-monitor watch";
            Restart = "on-failure";
            RestartSec = 5;
          };
          Install.WantedBy = [ "default.target" ];
        };

    # A respawn from the web view starts tmux and the agent from this unit's
    # environment, which has no login PATH, so the profile directories and the
    # two tools the library calls are named here.
    systemd.user.services.claude-monitor-serve =
      lib.mkIf (cfg.serve.enable && pkgs.stdenv.hostPlatform.isLinux)
        {
          Unit.Description = "claude-monitor: web view of AI agent sessions (loopback only)";
          Service = {
            ExecStart = "${cfg.package}/bin/claude-monitor serve --port ${toString cfg.serve.port}";
            Restart = "on-failure";
            RestartSec = 5;
            Environment = [
              "PATH=${
                lib.makeBinPath [
                  pkgs.tmux
                  pkgs.git
                ]
              }:%h/.nix-profile/bin:/etc/profiles/per-user/%u/bin:/run/current-system/sw/bin"
            ];
          };
          Install.WantedBy = [ "default.target" ];
        };

    systemd.user.services.claude-monitor-boot-check =
      lib.mkIf (cfg.bootCheck.enable && pkgs.stdenv.hostPlatform.isLinux)
        {
          Unit.Description = "claude-monitor: report AI sessions interrupted by a reboot";
          Service = {
            Type = "oneshot";
            ExecStart = "${cfg.package}/bin/claude-monitor boot-check";
          };
          Install.WantedBy = [ "default.target" ];
        };

    launchd.agents.claude-monitor-boot-check =
      lib.mkIf (cfg.bootCheck.enable && pkgs.stdenv.hostPlatform.isDarwin)
        {
          enable = true;
          config = {
            ProgramArguments = [
              "${cfg.package}/bin/claude-monitor"
              "boot-check"
            ];
            RunAtLoad = true;
          };
        };

    # Merged into the upstream home-manager `programs.claude-code.settings`
    # option (JSON-value type: attrsOf recurses, listOf concatenates), so this
    # adds one hook group per event alongside whatever the user's own config
    # already registers for the same event — it does not replace it.
    programs.claude-code.settings.hooks = lib.genAttrs hookEvents (event: [ (mkHookGroup event) ]);
  };
}
